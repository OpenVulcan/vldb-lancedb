use crate::types::{
    LanceDbColumnDef, LanceDbColumnType, LanceDbCreateTableInput, LanceDbCreateTableResult,
    LanceDbDeleteInput, LanceDbDeleteResult, LanceDbDropTableInput, LanceDbDropTableResult,
    LanceDbInputFormat, LanceDbOutputFormat, LanceDbSearchInput, LanceDbSearchResult,
    LanceDbUpsertInput, LanceDbUpsertResult,
};
use arrow_array::builder::{
    BooleanBuilder, FixedSizeListBuilder, Float32Builder, Float64Builder, Int32Builder,
    Int64Builder, LargeStringBuilder, StringBuilder, UInt32Builder, UInt64Builder,
};
use arrow_array::{
    Array, ArrayRef, BooleanArray, FixedSizeListArray, Float32Array, Float64Array, Int32Array,
    Int64Array, LargeStringArray, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
    UInt32Array, UInt64Array,
};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures::TryStreamExt;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::AddDataMode;
use lancedb::{Connection, Table};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, Semaphore};

/// 引擎错误类别，供调用方根据语义决定如何映射错误。
/// Engine error category so callers can map errors according to semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanceDbEngineErrorKind {
    InvalidArgument,
    Internal,
}

/// 引擎错误，描述库层执行失败的原因。
/// Engine error describing why a library-layer operation failed.
#[derive(Debug, Clone)]
pub struct LanceDbEngineError {
    pub kind: LanceDbEngineErrorKind,
    pub message: String,
}

impl LanceDbEngineError {
    /// 构造参数错误。
    /// Build an invalid-argument error.
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self {
            kind: LanceDbEngineErrorKind::InvalidArgument,
            message: message.into(),
        }
    }

    /// 构造内部错误。
    /// Build an internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: LanceDbEngineErrorKind::Internal,
            message: message.into(),
        }
    }
}

impl Display for LanceDbEngineError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LanceDbEngineError {}

/// 引擎配置，控制单实例的安全限制和并发行为。
/// Engine options controlling safety limits and concurrency for a single instance.
#[derive(Debug, Clone)]
pub struct LanceDbEngineOptions {
    pub max_upsert_payload: usize,
    pub max_search_limit: usize,
    pub max_concurrent_requests: usize,
}

impl Default for LanceDbEngineOptions {
    fn default() -> Self {
        Self {
            max_upsert_payload: 50 * 1024 * 1024,
            max_search_limit: 10_000,
            max_concurrent_requests: 500,
        }
    }
}

/// 可嵌入的 LanceDB 引擎实例，封装单库连接和核心操作。
/// Embeddable LanceDB engine instance encapsulating one database connection and core operations.
#[derive(Clone)]
pub struct LanceDbEngine {
    state: Arc<LanceDbEngineState>,
}

/// 引擎内部共享状态。
/// Internal shared state for the engine.
struct LanceDbEngineState {
    db: Arc<Connection>,
    table_access: TableAccessCoordinator,
    concurrency_limiter: Arc<Semaphore>,
    options: LanceDbEngineOptions,
}

/// 表级锁协调器，用于控制同表读写并发。
/// Table-level lock coordinator used to control per-table read/write concurrency.
#[derive(Default)]
struct TableAccessCoordinator {
    locks: Mutex<HashMap<String, Arc<RwLock<()>>>>,
}

impl TableAccessCoordinator {
    /// 获取某张表对应的锁对象。
    /// Get the lock object corresponding to a table.
    fn lock_for(&self, table_name: &str) -> Arc<RwLock<()>> {
        let mut guard = self
            .locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        Arc::clone(
            guard
                .entry(normalize_table_name(table_name))
                .or_insert_with(|| Arc::new(RwLock::new(()))),
        )
    }

    /// 获取表级读锁。
    /// Acquire a table-level read lock.
    async fn acquire_read(&self, table_name: &str) -> OwnedRwLockReadGuard<()> {
        self.lock_for(table_name).read_owned().await
    }

    /// 获取表级写锁。
    /// Acquire a table-level write lock.
    async fn acquire_write(&self, table_name: &str) -> OwnedRwLockWriteGuard<()> {
        self.lock_for(table_name).write_owned().await
    }
}

impl LanceDbEngine {
    /// 使用现有连接创建引擎实例。
    /// Create an engine instance from an existing connection.
    pub fn new(db: Arc<Connection>, options: LanceDbEngineOptions) -> Self {
        let max = options.max_concurrent_requests.max(1);
        Self {
            state: Arc::new(LanceDbEngineState {
                db,
                table_access: TableAccessCoordinator::default(),
                concurrency_limiter: Arc::new(Semaphore::new(max)),
                options,
            }),
        }
    }

    /// 使用非共享连接创建引擎实例。
    /// Create an engine instance from a non-shared connection.
    pub fn from_connection(db: Connection, options: LanceDbEngineOptions) -> Self {
        Self::new(Arc::new(db), options)
    }

    /// 返回底层共享连接，便于调用方在必要时直接访问。
    /// Return the underlying shared connection so callers can access it directly when necessary.
    pub fn connection(&self) -> Arc<Connection> {
        Arc::clone(&self.state.db)
    }

    /// 创建向量表。
    /// Create a vector table.
    pub async fn create_table(
        &self,
        input: LanceDbCreateTableInput,
    ) -> Result<LanceDbCreateTableResult, LanceDbEngineError> {
        let table_name = normalize_table_name(&input.table_name);
        if table_name.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "table_name must not be empty",
            ));
        }
        if input.columns.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "columns must not be empty",
            ));
        }

        let schema = build_arrow_schema(&input.columns)?;
        self.acquire_concurrency_permit().await?;

        let _table_guard = self.acquire_table_write(&table_name).await;
        let mut builder = self.state.db.create_empty_table(table_name.clone(), schema);
        builder = if input.overwrite_if_exists {
            builder.mode(CreateTableMode::Overwrite)
        } else {
            builder.mode(CreateTableMode::Create)
        };
        builder.execute().await.map_err(to_engine_error)?;

        Ok(LanceDbCreateTableResult {
            message: format!("table '{}' is ready", table_name),
        })
    }

    /// 追加或合并写入数据。
    /// Append or merge-insert data into a table.
    pub async fn vector_upsert(
        &self,
        input: LanceDbUpsertInput,
    ) -> Result<LanceDbUpsertResult, LanceDbEngineError> {
        let table_name = normalize_table_name(&input.table_name);
        if table_name.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "table_name must not be empty",
            ));
        }
        if input.data.len() > self.state.options.max_upsert_payload {
            return Err(LanceDbEngineError::invalid_argument(format!(
                "payload too large: {} bytes (max {})",
                input.data.len(),
                self.state.options.max_upsert_payload
            )));
        }

        self.acquire_concurrency_permit().await?;
        let _table_guard = self.acquire_table_write(&table_name).await;
        let table = self.open_table(&table_name).await?;
        let schema = table.schema().await.map_err(to_engine_error)?;

        let decode_format = input.input_format;
        let decode_data = input.data.clone();
        let decode_schema = schema.clone();
        let (batches, input_rows) = tokio::task::spawn_blocking(move || {
            decode_input_to_batches(decode_format, &decode_data, decode_schema)
        })
        .await
        .map_err(|e| LanceDbEngineError::internal(format!("decode task panicked: {e}")))??;

        if input_rows == 0 {
            let version = table.version().await.map_err(to_engine_error)?;
            return Ok(LanceDbUpsertResult {
                message: "no rows to write".to_string(),
                version,
                input_rows: 0,
                inserted_rows: 0,
                updated_rows: 0,
                deleted_rows: 0,
            });
        }

        let schema = table.schema().await.map_err(to_engine_error)?;
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(RecordBatchIterator::new(
            batches.into_iter().map(Ok),
            schema,
        ));

        if input.key_columns.is_empty() {
            table
                .add(reader)
                .mode(AddDataMode::Append)
                .execute()
                .await
                .map_err(to_engine_error)?;

            Ok(LanceDbUpsertResult {
                message: "append completed".to_string(),
                version: table.version().await.map_err(to_engine_error)?,
                input_rows,
                inserted_rows: input_rows,
                updated_rows: 0,
                deleted_rows: 0,
            })
        } else {
            let keys = input
                .key_columns
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let mut merge = table.merge_insert(&keys);
            merge
                .when_matched_update_all(None)
                .when_not_matched_insert_all();

            let result = merge.execute(reader).await.map_err(to_engine_error)?;
            Ok(LanceDbUpsertResult {
                message: "merge upsert completed".to_string(),
                version: result.version,
                input_rows,
                inserted_rows: result.num_inserted_rows,
                updated_rows: result.num_updated_rows,
                deleted_rows: result.num_deleted_rows,
            })
        }
    }

    /// 执行向量检索。
    /// Execute a vector search.
    pub async fn vector_search(
        &self,
        input: LanceDbSearchInput,
    ) -> Result<LanceDbSearchResult, LanceDbEngineError> {
        let table_name = normalize_table_name(&input.table_name);
        if table_name.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "table_name must not be empty",
            ));
        }
        if input.vector.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "vector must not be empty",
            ));
        }

        self.acquire_concurrency_permit().await?;
        let (batches, output_schema) = {
            let _table_guard = self.acquire_table_read(&table_name).await;
            let table = self.open_table(&table_name).await?;
            let mut query = table
                .query()
                .nearest_to(input.vector.clone())
                .map_err(to_engine_error)?;

            if !input.vector_column.trim().is_empty() {
                query = query.column(input.vector_column.trim());
            }

            let limit = if input.limit == 0 {
                10
            } else {
                (input.limit as usize).min(self.state.options.max_search_limit)
            };
            query = query.limit(limit);

            if !input.filter.trim().is_empty() {
                query = query.only_if(input.filter.trim().to_string());
            }

            let output_schema = query.output_schema().await.map_err(to_engine_error)?;
            let stream = query.execute().await.map_err(to_engine_error)?;
            let batches: Vec<RecordBatch> = stream.try_collect().await.map_err(to_engine_error)?;
            (batches, output_schema)
        };

        let rows = count_rows(&batches);
        let output_format = normalize_output_format(input.output_format);
        let data = match output_format {
            LanceDbOutputFormat::JsonRows => {
                let schema = output_schema.clone();
                let batches_ref = batches.clone();
                tokio::task::spawn_blocking(move || encode_batches_as_json(&schema, &batches_ref))
                    .await
                    .map_err(|e| {
                        LanceDbEngineError::internal(format!("encoding task panicked: {e}"))
                    })??
            }
            LanceDbOutputFormat::Unspecified | LanceDbOutputFormat::ArrowIpc => {
                let schema = output_schema.clone();
                let batches_ref = batches.clone();
                tokio::task::spawn_blocking(move || {
                    encode_batches_as_arrow_ipc(&schema, &batches_ref)
                })
                .await
                .map_err(|e| {
                    LanceDbEngineError::internal(format!("encoding task panicked: {e}"))
                })??
            }
        };

        Ok(LanceDbSearchResult {
            message: "search completed".to_string(),
            format: output_format,
            rows,
            data,
        })
    }

    /// 按条件删除记录。
    /// Delete rows by predicate.
    pub async fn delete(
        &self,
        input: LanceDbDeleteInput,
    ) -> Result<LanceDbDeleteResult, LanceDbEngineError> {
        let table_name = normalize_table_name(&input.table_name);
        if table_name.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "table_name must not be empty",
            ));
        }
        if input.condition.trim().is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "condition must not be empty",
            ));
        }

        self.acquire_concurrency_permit().await?;
        let _table_guard = self.acquire_table_write(&table_name).await;
        let table = self.open_table(&table_name).await?;
        let result = table
            .delete(input.condition.trim())
            .await
            .map_err(to_engine_error)?;

        Ok(LanceDbDeleteResult {
            message: format!("delete completed for '{}'", table_name),
            version: result.version,
            deleted_rows: result.num_deleted_rows,
        })
    }

    /// 删除整张表。
    /// Drop an entire table.
    pub async fn drop_table(
        &self,
        input: LanceDbDropTableInput,
    ) -> Result<LanceDbDropTableResult, LanceDbEngineError> {
        let table_name = normalize_table_name(&input.table_name);
        if table_name.is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "table_name must not be empty",
            ));
        }

        self.acquire_concurrency_permit().await?;
        let _table_guard = self.acquire_table_write(&table_name).await;
        self.state
            .db
            .drop_table(table_name.clone(), &[])
            .await
            .map_err(to_engine_error)?;

        Ok(LanceDbDropTableResult {
            message: format!("table '{}' dropped", table_name),
        })
    }

    /// 获取表级读锁。
    /// Acquire a table-level read lock.
    async fn acquire_table_read(&self, table_name: &str) -> OwnedRwLockReadGuard<()> {
        self.state.table_access.acquire_read(table_name).await
    }

    /// 获取表级写锁。
    /// Acquire a table-level write lock.
    async fn acquire_table_write(&self, table_name: &str) -> OwnedRwLockWriteGuard<()> {
        self.state.table_access.acquire_write(table_name).await
    }

    /// 获取并发令牌。
    /// Acquire the concurrency permit.
    async fn acquire_concurrency_permit(&self) -> Result<(), LanceDbEngineError> {
        self.state
            .concurrency_limiter
            .acquire()
            .await
            .map(|_| ())
            .map_err(|_| LanceDbEngineError::internal("engine is shutting down"))
    }

    /// 打开表对象。
    /// Open a table handle.
    async fn open_table(&self, table_name: &str) -> Result<Table, LanceDbEngineError> {
        self.state
            .db
            .open_table(table_name.to_string())
            .execute()
            .await
            .map_err(to_engine_error)
    }
}

/// 将表名标准化，避免锁与真实表名不一致。
/// Normalize the table name so locks and real table names stay aligned.
fn normalize_table_name(table_name: &str) -> String {
    table_name.trim().to_string()
}

/// 构建 Arrow Schema。
/// Build the Arrow schema.
fn build_arrow_schema(columns: &[LanceDbColumnDef]) -> Result<SchemaRef, LanceDbEngineError> {
    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        if column.name.trim().is_empty() {
            return Err(LanceDbEngineError::invalid_argument(
                "column name must not be empty",
            ));
        }
        fields.push(column_to_field(column)?);
    }
    Ok(Arc::new(Schema::new(fields)))
}

/// 将列定义转换成 Arrow Field。
/// Convert the column definition into an Arrow field.
fn column_to_field(column: &LanceDbColumnDef) -> Result<Field, LanceDbEngineError> {
    let data_type = match column.column_type {
        LanceDbColumnType::String => DataType::Utf8,
        LanceDbColumnType::Int64 => DataType::Int64,
        LanceDbColumnType::Float64 => DataType::Float64,
        LanceDbColumnType::Bool => DataType::Boolean,
        LanceDbColumnType::Float32 => DataType::Float32,
        LanceDbColumnType::Uint64 => DataType::UInt64,
        LanceDbColumnType::Int32 => DataType::Int32,
        LanceDbColumnType::Uint32 => DataType::UInt32,
        LanceDbColumnType::VectorFloat32 => {
            if column.vector_dim == 0 {
                return Err(LanceDbEngineError::invalid_argument(format!(
                    "vector column '{}' must have vector_dim > 0",
                    column.name
                )));
            }
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                column.vector_dim as i32,
            )
        }
        LanceDbColumnType::Unspecified => {
            return Err(LanceDbEngineError::invalid_argument(format!(
                "column '{}' has unspecified type",
                column.name
            )));
        }
    };

    Ok(Field::new(&column.name, data_type, column.nullable))
}

/// 解码输入载荷。
/// Decode the input payload.
fn decode_input_to_batches(
    format: LanceDbInputFormat,
    data: &[u8],
    schema: SchemaRef,
) -> Result<(Vec<RecordBatch>, u64), LanceDbEngineError> {
    match format {
        LanceDbInputFormat::JsonRows | LanceDbInputFormat::Unspecified => {
            decode_json_rows_to_batches(data, schema)
        }
        LanceDbInputFormat::ArrowIpc => decode_arrow_ipc_to_batches(data),
    }
}

/// 解码 Arrow IPC 载荷。
/// Decode the Arrow IPC payload.
fn decode_arrow_ipc_to_batches(data: &[u8]) -> Result<(Vec<RecordBatch>, u64), LanceDbEngineError> {
    let mut reader =
        StreamReader::try_new(Cursor::new(data.to_vec()), None).map_err(to_engine_error)?;
    let mut batches = Vec::new();
    let mut rows = 0_u64;

    for batch in &mut reader {
        let batch = batch.map_err(to_engine_error)?;
        rows += batch.num_rows() as u64;
        batches.push(batch);
    }

    Ok((batches, rows))
}

/// 解码 JSON Rows 载荷。
/// Decode the JSON Rows payload.
fn decode_json_rows_to_batches(
    data: &[u8],
    schema: SchemaRef,
) -> Result<(Vec<RecordBatch>, u64), LanceDbEngineError> {
    let rows: Vec<Value> = if data.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice(data).map_err(|e| {
            LanceDbEngineError::invalid_argument(format!(
                "failed to parse JSON rows, expected a JSON array of objects: {e}"
            ))
        })?
    };

    let batch = json_rows_to_record_batch(&rows, schema)?;
    let row_count = batch.num_rows() as u64;
    Ok((vec![batch], row_count))
}

/// 将 JSON 行数据转换为 RecordBatch。
/// Convert JSON row data into a RecordBatch.
fn json_rows_to_record_batch(
    rows: &[Value],
    schema: SchemaRef,
) -> Result<RecordBatch, LanceDbEngineError> {
    let mut arrays = Vec::<ArrayRef>::with_capacity(schema.fields().len());

    for field in schema.fields() {
        arrays.push(build_array_for_field(
            rows,
            field.name(),
            field.data_type(),
            field.is_nullable(),
        )?);
    }

    RecordBatch::try_new(schema, arrays).map_err(to_engine_error)
}

/// 为一个字段构建 Arrow 数组。
/// Build the Arrow array for one field.
fn build_array_for_field(
    rows: &[Value],
    field_name: &str,
    data_type: &DataType,
    nullable: bool,
) -> Result<ArrayRef, LanceDbEngineError> {
    match data_type {
        DataType::Utf8 => {
            let mut builder = StringBuilder::new();
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_string(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::LargeUtf8 => {
            let mut builder = LargeStringBuilder::new();
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_string(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Int64 => {
            let mut builder = Int64Builder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_i64(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Int32 => {
            let mut builder = Int32Builder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_i32(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::UInt64 => {
            let mut builder = UInt64Builder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_u64(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::UInt32 => {
            let mut builder = UInt32Builder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_u32(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Float64 => {
            let mut builder = Float64Builder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_f64(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Float32 => {
            let mut builder = Float32Builder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_f32(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Boolean => {
            let mut builder = BooleanBuilder::with_capacity(rows.len());
            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => builder.append_value(expect_bool(value, field_name)?),
                    None => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::FixedSizeList(child, dim) if child.data_type() == &DataType::Float32 => {
            let mut builder = FixedSizeListBuilder::with_capacity(
                Float32Builder::with_capacity(rows.len() * (*dim as usize)),
                *dim,
                rows.len(),
            );

            for row in rows {
                match extract_field_value(row, field_name, nullable)? {
                    Some(value) => {
                        let array = value.as_array().ok_or_else(|| {
                            LanceDbEngineError::invalid_argument(format!(
                                "field '{}' must be a JSON array of float32 values",
                                field_name
                            ))
                        })?;
                        if array.len() != *dim as usize {
                            return Err(LanceDbEngineError::invalid_argument(format!(
                                "field '{}' length mismatch: expected {}, got {}",
                                field_name,
                                dim,
                                array.len()
                            )));
                        }
                        for item in array {
                            builder.values().append_value(expect_f32(item, field_name)?);
                        }
                        builder.append(true);
                    }
                    None => {
                        for _ in 0..*dim {
                            builder.values().append_null();
                        }
                        builder.append(false);
                    }
                }
            }

            Ok(Arc::new(builder.finish()))
        }
        other => Err(LanceDbEngineError::invalid_argument(format!(
            "unsupported field type for JSON ingestion on '{}': {:?}",
            field_name, other
        ))),
    }
}

/// 从 JSON 对象中提取字段值。
/// Extract the field value from a JSON object.
fn extract_field_value<'a>(
    row: &'a Value,
    field_name: &str,
    nullable: bool,
) -> Result<Option<&'a Value>, LanceDbEngineError> {
    let object = row.as_object().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(
            "JSON rows must be an array of JSON objects".to_string(),
        )
    })?;

    match object.get(field_name) {
        Some(Value::Null) => {
            if nullable {
                Ok(None)
            } else {
                Err(LanceDbEngineError::invalid_argument(format!(
                    "field '{}' is not nullable",
                    field_name
                )))
            }
        }
        Some(value) => Ok(Some(value)),
        None => {
            if nullable {
                Ok(None)
            } else {
                Err(LanceDbEngineError::invalid_argument(format!(
                    "field '{}' is missing and not nullable",
                    field_name
                )))
            }
        }
    }
}

/// 期望字符串值。
/// Expect a string value.
fn expect_string<'a>(value: &'a Value, field_name: &str) -> Result<&'a str, LanceDbEngineError> {
    value.as_str().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be a string", field_name))
    })
}

/// 期望 i64 值。
/// Expect an i64 value.
fn expect_i64(value: &Value, field_name: &str) -> Result<i64, LanceDbEngineError> {
    value.as_i64().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be an int64", field_name))
    })
}

/// 期望 i32 值。
/// Expect an i32 value.
fn expect_i32(value: &Value, field_name: &str) -> Result<i32, LanceDbEngineError> {
    let raw = value.as_i64().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be an int32", field_name))
    })?;
    i32::try_from(raw).map_err(|_| {
        LanceDbEngineError::invalid_argument(format!(
            "field '{}' is out of int32 range",
            field_name
        ))
    })
}

/// 期望 u64 值。
/// Expect a u64 value.
fn expect_u64(value: &Value, field_name: &str) -> Result<u64, LanceDbEngineError> {
    value.as_u64().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be a uint64", field_name))
    })
}

/// 期望 u32 值。
/// Expect a u32 value.
fn expect_u32(value: &Value, field_name: &str) -> Result<u32, LanceDbEngineError> {
    let raw = value.as_u64().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be a uint32", field_name))
    })?;
    u32::try_from(raw).map_err(|_| {
        LanceDbEngineError::invalid_argument(format!(
            "field '{}' is out of uint32 range",
            field_name
        ))
    })
}

/// 期望 f64 值。
/// Expect a f64 value.
fn expect_f64(value: &Value, field_name: &str) -> Result<f64, LanceDbEngineError> {
    value.as_f64().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be a float64", field_name))
    })
}

/// 期望 f32 值。
/// Expect a f32 value.
fn expect_f32(value: &Value, field_name: &str) -> Result<f32, LanceDbEngineError> {
    let raw = value.as_f64().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be a float32", field_name))
    })?;
    Ok(raw as f32)
}

/// 期望布尔值。
/// Expect a boolean value.
fn expect_bool(value: &Value, field_name: &str) -> Result<bool, LanceDbEngineError> {
    value.as_bool().ok_or_else(|| {
        LanceDbEngineError::invalid_argument(format!("field '{}' must be a bool", field_name))
    })
}

/// 将 RecordBatch 编码为 Arrow IPC。
/// Encode record batches into Arrow IPC.
fn encode_batches_as_arrow_ipc(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, LanceDbEngineError> {
    let mut writer =
        StreamWriter::try_new(Vec::<u8>::new(), schema.as_ref()).map_err(to_engine_error)?;
    for batch in batches {
        writer.write(batch).map_err(to_engine_error)?;
    }
    writer.finish().map_err(to_engine_error)?;
    writer.into_inner().map_err(to_engine_error)
}

/// 将 RecordBatch 编码为 JSON 数组。
/// Encode record batches into a JSON array.
fn encode_batches_as_json(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, LanceDbEngineError> {
    let mut rows = Vec::<Value>::new();
    for batch in batches {
        for row_idx in 0..batch.num_rows() {
            let mut object = Map::<String, Value>::new();
            for (col_idx, field) in schema.fields().iter().enumerate() {
                let value = json_value_from_array(batch.column(col_idx), row_idx)?;
                object.insert(field.name().clone(), value);
            }
            rows.push(Value::Object(object));
        }
    }

    serde_json::to_vec(&rows).map_err(to_engine_error)
}

/// 从 Arrow 数组中取出 JSON 值。
/// Extract a JSON value from an Arrow array.
fn json_value_from_array(array: &ArrayRef, row_idx: usize) -> Result<Value, LanceDbEngineError> {
    if array.is_null(row_idx) {
        return Ok(Value::Null);
    }

    match array.data_type() {
        DataType::Utf8 => {
            let arr = array
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast Utf8 array"))?;
            Ok(Value::String(arr.value(row_idx).to_string()))
        }
        DataType::LargeUtf8 => {
            let arr = array
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .ok_or_else(|| {
                    LanceDbEngineError::internal("failed to downcast LargeUtf8 array")
                })?;
            Ok(Value::String(arr.value(row_idx).to_string()))
        }
        DataType::Int64 => {
            let arr = array
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast Int64 array"))?;
            Ok(Value::from(arr.value(row_idx)))
        }
        DataType::Int32 => {
            let arr = array
                .as_any()
                .downcast_ref::<Int32Array>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast Int32 array"))?;
            Ok(Value::from(arr.value(row_idx)))
        }
        DataType::UInt64 => {
            let arr = array
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast UInt64 array"))?;
            Ok(Value::from(arr.value(row_idx)))
        }
        DataType::UInt32 => {
            let arr = array
                .as_any()
                .downcast_ref::<UInt32Array>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast UInt32 array"))?;
            Ok(Value::from(arr.value(row_idx)))
        }
        DataType::Float64 => {
            let arr = array
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast Float64 array"))?;
            Ok(Value::from(arr.value(row_idx)))
        }
        DataType::Float32 => {
            let arr = array
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast Float32 array"))?;
            Ok(Value::from(arr.value(row_idx) as f64))
        }
        DataType::Boolean => {
            let arr = array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .ok_or_else(|| LanceDbEngineError::internal("failed to downcast Boolean array"))?;
            Ok(Value::from(arr.value(row_idx)))
        }
        DataType::FixedSizeList(child, _) if child.data_type() == &DataType::Float32 => {
            let arr = array
                .as_any()
                .downcast_ref::<FixedSizeListArray>()
                .ok_or_else(|| {
                    LanceDbEngineError::internal("failed to downcast FixedSizeList array")
                })?;
            let values = arr.value(row_idx);
            let floats = values
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| {
                    LanceDbEngineError::internal(
                        "failed to downcast FixedSizeList child Float32 array",
                    )
                })?;

            let mut items = Vec::with_capacity(floats.len());
            for idx in 0..floats.len() {
                if floats.is_null(idx) {
                    items.push(Value::Null);
                } else {
                    items.push(Value::from(floats.value(idx) as f64));
                }
            }
            Ok(Value::Array(items))
        }
        other => Err(LanceDbEngineError::internal(format!(
            "unsupported output type for JSON encoding: {:?}",
            other
        ))),
    }
}

/// 统计 RecordBatch 总行数。
/// Count total rows of the record batches.
fn count_rows(batches: &[RecordBatch]) -> u64 {
    batches.iter().map(|b| b.num_rows() as u64).sum()
}

/// 规范化检索输出格式，确保未指定时也能返回稳定的真实编码格式。
/// Normalize the search output format so an unspecified request still reports the stable effective encoding.
fn normalize_output_format(output_format: LanceDbOutputFormat) -> LanceDbOutputFormat {
    match output_format {
        LanceDbOutputFormat::Unspecified => LanceDbOutputFormat::ArrowIpc,
        other => other,
    }
}

/// 将显示型错误统一映射成引擎内部错误。
/// Map displayable errors into engine internal errors.
fn to_engine_error<E: Display>(error: E) -> LanceDbEngineError {
    LanceDbEngineError::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        LanceDbEngineOptions, TableAccessCoordinator, normalize_output_format, normalize_table_name,
    };
    use crate::types::LanceDbOutputFormat;
    use std::sync::Arc;

    #[test]
    fn normalize_table_name_trims_whitespace() {
        assert_eq!(normalize_table_name("  demo  "), "demo");
    }

    #[test]
    fn engine_options_default_values_are_stable() {
        let options = LanceDbEngineOptions::default();
        assert_eq!(options.max_upsert_payload, 50 * 1024 * 1024);
        assert_eq!(options.max_search_limit, 10_000);
        assert_eq!(options.max_concurrent_requests, 500);
    }

    #[test]
    fn normalize_output_format_defaults_to_arrow_ipc() {
        assert_eq!(
            normalize_output_format(LanceDbOutputFormat::Unspecified),
            LanceDbOutputFormat::ArrowIpc
        );
        assert_eq!(
            normalize_output_format(LanceDbOutputFormat::JsonRows),
            LanceDbOutputFormat::JsonRows
        );
    }

    #[tokio::test]
    async fn table_access_coordinator_reuses_trimmed_table_lock() {
        let coordinator = TableAccessCoordinator::default();
        let lock_a = coordinator.lock_for("demo");
        let lock_b = coordinator.lock_for(" demo ");
        assert!(Arc::ptr_eq(&lock_a, &lock_b));
    }

    #[tokio::test]
    async fn table_write_waits_for_active_reader() {
        let coordinator = Arc::new(TableAccessCoordinator::default());
        let reader_guard = coordinator.acquire_read("demo").await;
        let writer = {
            let coordinator = Arc::clone(&coordinator);
            tokio::spawn(async move {
                let _writer_guard = coordinator.acquire_write(" demo ").await;
            })
        };

        tokio::task::yield_now().await;
        assert!(!writer.is_finished());

        drop(reader_guard);
        writer.await.expect("writer task should complete");
    }
}
