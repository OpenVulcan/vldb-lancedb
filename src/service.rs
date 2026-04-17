use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tonic::{Request, Response, Status};
use vldb_lancedb::engine::{LanceDbEngine, LanceDbEngineError, LanceDbEngineErrorKind};
use vldb_lancedb::types::{
    LanceDbColumnDef, LanceDbColumnType, LanceDbCreateTableInput, LanceDbDeleteInput,
    LanceDbDropTableInput, LanceDbInputFormat, LanceDbOutputFormat, LanceDbSearchInput,
    LanceDbUpsertInput,
};

use crate::logging::{LoggingConfig, ServiceLogger};
use crate::pb::lance_db_service_server::LanceDbService;
use crate::pb::{
    ColumnDef, ColumnType, CreateTableRequest, CreateTableResponse, DeleteRequest, DeleteResponse,
    DropTableRequest, DropTableResponse, InputFormat, OutputFormat, SearchRequest, SearchResponse,
    UpsertRequest, UpsertResponse,
};

/// gRPC 服务配置，仅保留传输层需要的控制项。
/// gRPC service configuration keeping only transport-layer control settings.
pub struct ServiceConfig {
    pub request_timeout: Option<Duration>,
}

/// gRPC 服务实现，负责日志、超时与 protobuf/engine 之间的映射。
/// gRPC service implementation responsible for logging, timeouts, and protobuf-to-engine mapping.
#[derive(Clone)]
pub struct LanceDbGrpcService {
    state: Arc<ServiceState>,
}

/// 服务内部状态。
/// Internal service state.
struct ServiceState {
    engine: LanceDbEngine,
    logger: Arc<ServiceLogger>,
    config: ServiceConfig,
}

/// 请求日志上下文。
/// Request logging context.
#[derive(Clone, Debug)]
struct RequestLogContext {
    request_id: u64,
    operation: &'static str,
    remote_addr: String,
    summary: String,
    started_at: Instant,
    logger: Arc<ServiceLogger>,
    request_log_enabled: bool,
    slow_request_log_enabled: bool,
    slow_request_threshold: Duration,
    include_request_details_in_slow_log: bool,
}

impl LanceDbGrpcService {
    /// 使用已经构造好的引擎创建 gRPC 服务实例。
    /// Create the gRPC service instance from an already constructed engine.
    pub fn from_engine(
        engine: LanceDbEngine,
        logger: Arc<ServiceLogger>,
        config: ServiceConfig,
    ) -> Self {
        Self {
            state: Arc::new(ServiceState {
                engine,
                logger,
                config,
            }),
        }
    }

    /// 用可选超时包装 Future。
    /// Wrap a future with an optional timeout.
    async fn with_timeout<R>(
        &self,
        future: impl std::future::Future<Output = R>,
    ) -> Result<R, Status> {
        match &self.state.config.request_timeout {
            Some(timeout) => tokio::time::timeout(*timeout, future).await.map_err(|_| {
                Status::deadline_exceeded(format!("request timeout after {:?}", *timeout))
            }),
            None => Ok(future.await),
        }
    }
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[tonic::async_trait]
impl LanceDbService for LanceDbGrpcService {
    async fn create_table(
        &self,
        request: Request<CreateTableRequest>,
    ) -> Result<Response<CreateTableResponse>, Status> {
        let context = build_request_context(
            &self.state.logger,
            "create_table",
            request.remote_addr(),
            format!(
                "table={} columns={} overwrite_if_exists={}",
                request.get_ref().table_name.trim(),
                request.get_ref().columns.len(),
                request.get_ref().overwrite_if_exists,
            ),
        );
        log_request_started(&context);
        let req = request.into_inner();

        let input = LanceDbCreateTableInput {
            table_name: req.table_name,
            columns: req.columns.iter().map(map_column_def).collect(),
            overwrite_if_exists: req.overwrite_if_exists,
        };

        match self
            .with_timeout(self.state.engine.create_table(input))
            .await?
        {
            Ok(result) => {
                log_request_succeeded(&context, result.message.as_str());
                Ok(Response::new(CreateTableResponse {
                    success: true,
                    message: result.message,
                }))
            }
            Err(error) => {
                let status = map_engine_error(error);
                log_request_failed(&context, &status);
                Err(status)
            }
        }
    }

    async fn vector_upsert(
        &self,
        request: Request<UpsertRequest>,
    ) -> Result<Response<UpsertResponse>, Status> {
        let context = build_request_context(
            &self.state.logger,
            "vector_upsert",
            request.remote_addr(),
            format!(
                "table={} key_columns={} input_format={:?} payload_bytes={}",
                request.get_ref().table_name.trim(),
                request.get_ref().key_columns.len(),
                request.get_ref().input_format(),
                request.get_ref().data.len(),
            ),
        );
        log_request_started(&context);
        let req = request.into_inner();
        let input_format = map_input_format(req.input_format());

        let input = LanceDbUpsertInput {
            table_name: req.table_name,
            input_format,
            data: req.data,
            key_columns: req.key_columns,
        };

        match self
            .with_timeout(self.state.engine.vector_upsert(input))
            .await?
        {
            Ok(result) => {
                log_request_succeeded(&context, result.message.as_str());
                Ok(Response::new(UpsertResponse {
                    success: true,
                    message: result.message,
                    version: result.version,
                    input_rows: result.input_rows,
                    inserted_rows: result.inserted_rows,
                    updated_rows: result.updated_rows,
                    deleted_rows: result.deleted_rows,
                }))
            }
            Err(error) => {
                let status = map_engine_error(error);
                log_request_failed(&context, &status);
                Err(status)
            }
        }
    }

    async fn vector_search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        let context = build_request_context(
            &self.state.logger,
            "vector_search",
            request.remote_addr(),
            format!(
                "table={} vector_dim={} limit={} output_format={:?} filter=\"{}\"",
                request.get_ref().table_name.trim(),
                request.get_ref().vector.len(),
                request.get_ref().limit,
                request.get_ref().output_format(),
                preview_text(
                    request.get_ref().filter.trim(),
                    self.state.logger.config().request_preview_chars
                ),
            ),
        );
        log_request_started(&context);
        let req = request.into_inner();
        let output_format = map_output_format(req.output_format());

        let input = LanceDbSearchInput {
            table_name: req.table_name,
            vector: req.vector,
            limit: req.limit,
            filter: req.filter,
            vector_column: req.vector_column,
            output_format,
        };

        match self
            .with_timeout(self.state.engine.vector_search(input))
            .await?
        {
            Ok(result) => {
                log_request_succeeded(
                    &context,
                    format!(
                        "{} rows encoded as {}",
                        result.rows,
                        result.format.as_wire_name()
                    ),
                );
                Ok(Response::new(SearchResponse {
                    success: true,
                    message: result.message,
                    format: result.format.as_wire_name().to_string(),
                    rows: result.rows,
                    data: result.data,
                }))
            }
            Err(error) => {
                let status = map_engine_error(error);
                log_request_failed(&context, &status);
                Err(status)
            }
        }
    }

    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let context = build_request_context(
            &self.state.logger,
            "delete",
            request.remote_addr(),
            format!(
                "table={} condition=\"{}\"",
                request.get_ref().table_name.trim(),
                preview_text(
                    request.get_ref().condition.trim(),
                    self.state.logger.config().request_preview_chars
                ),
            ),
        );
        log_request_started(&context);
        let req = request.into_inner();

        let input = LanceDbDeleteInput {
            table_name: req.table_name,
            condition: req.condition,
        };

        match self.with_timeout(self.state.engine.delete(input)).await? {
            Ok(result) => {
                log_request_succeeded(&context, format!("deleted_rows={}", result.deleted_rows));
                Ok(Response::new(DeleteResponse {
                    success: true,
                    message: result.message,
                    version: result.version,
                    deleted_rows: result.deleted_rows,
                }))
            }
            Err(error) => {
                let status = map_engine_error(error);
                log_request_failed(&context, &status);
                Err(status)
            }
        }
    }

    async fn drop_table(
        &self,
        request: Request<DropTableRequest>,
    ) -> Result<Response<DropTableResponse>, Status> {
        let context = build_request_context(
            &self.state.logger,
            "drop_table",
            request.remote_addr(),
            format!("table={}", request.get_ref().table_name.trim()),
        );
        log_request_started(&context);
        let req = request.into_inner();

        let input = LanceDbDropTableInput {
            table_name: req.table_name,
        };

        match self
            .with_timeout(self.state.engine.drop_table(input))
            .await?
        {
            Ok(result) => {
                log_request_succeeded(&context, result.message.as_str());
                Ok(Response::new(DropTableResponse {
                    success: true,
                    message: result.message,
                }))
            }
            Err(error) => {
                let status = map_engine_error(error);
                log_request_failed(&context, &status);
                Err(status)
            }
        }
    }
}

/// 将 protobuf 列定义映射到库层列定义。
/// Map a protobuf column definition into the library-layer column definition.
fn map_column_def(column: &ColumnDef) -> LanceDbColumnDef {
    LanceDbColumnDef {
        name: column.name.clone(),
        column_type: map_column_type(column.column_type()),
        vector_dim: column.vector_dim,
        nullable: column.nullable,
    }
}

/// 将 protobuf 列类型映射到库层列类型。
/// Map a protobuf column type into the library-layer column type.
fn map_column_type(column_type: ColumnType) -> LanceDbColumnType {
    match column_type {
        ColumnType::String => LanceDbColumnType::String,
        ColumnType::Int64 => LanceDbColumnType::Int64,
        ColumnType::Float64 => LanceDbColumnType::Float64,
        ColumnType::Bool => LanceDbColumnType::Bool,
        ColumnType::VectorFloat32 => LanceDbColumnType::VectorFloat32,
        ColumnType::Float32 => LanceDbColumnType::Float32,
        ColumnType::Uint64 => LanceDbColumnType::Uint64,
        ColumnType::Int32 => LanceDbColumnType::Int32,
        ColumnType::Uint32 => LanceDbColumnType::Uint32,
        ColumnType::Unspecified => LanceDbColumnType::Unspecified,
    }
}

/// 将 protobuf 输入格式映射到库层输入格式。
/// Map a protobuf input format into the library-layer input format.
fn map_input_format(input_format: InputFormat) -> LanceDbInputFormat {
    match input_format {
        InputFormat::JsonRows => LanceDbInputFormat::JsonRows,
        InputFormat::ArrowIpc => LanceDbInputFormat::ArrowIpc,
        InputFormat::Unspecified => LanceDbInputFormat::Unspecified,
    }
}

/// 将 protobuf 输出格式映射到库层输出格式。
/// Map a protobuf output format into the library-layer output format.
fn map_output_format(output_format: OutputFormat) -> LanceDbOutputFormat {
    match output_format {
        OutputFormat::JsonRows => LanceDbOutputFormat::JsonRows,
        OutputFormat::ArrowIpc => LanceDbOutputFormat::ArrowIpc,
        OutputFormat::Unspecified => LanceDbOutputFormat::Unspecified,
    }
}

/// 将引擎错误映射为 gRPC Status。
/// Map an engine error into a gRPC status.
fn map_engine_error(error: LanceDbEngineError) -> Status {
    match error.kind {
        LanceDbEngineErrorKind::InvalidArgument => Status::invalid_argument(error.message),
        LanceDbEngineErrorKind::Internal => Status::internal(error.message),
    }
}

/// 构建请求日志上下文。
/// Build the request logging context.
fn build_request_context(
    logger: &Arc<ServiceLogger>,
    operation: &'static str,
    remote_addr: Option<std::net::SocketAddr>,
    summary: String,
) -> RequestLogContext {
    let logging: &LoggingConfig = logger.config();
    RequestLogContext {
        request_id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
        operation,
        remote_addr: remote_addr
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        summary,
        started_at: Instant::now(),
        logger: Arc::clone(logger),
        request_log_enabled: logging.request_log_enabled,
        slow_request_log_enabled: logging.slow_request_log_enabled,
        slow_request_threshold: Duration::from_millis(logging.slow_request_threshold_ms),
        include_request_details_in_slow_log: logging.include_request_details_in_slow_log,
    }
}

/// 记录请求开始日志。
/// Record the request-start log.
fn log_request_started(context: &RequestLogContext) {
    if !context.request_log_enabled {
        return;
    }

    context.logger.log(
        "start",
        format!(
            "request_id={} op={} remote={} summary={}",
            context.request_id, context.operation, context.remote_addr, context.summary
        ),
    );
}

/// 记录请求成功日志。
/// Record the request-success log.
fn log_request_succeeded(context: &RequestLogContext, detail: impl AsRef<str>) {
    let elapsed = context.started_at.elapsed();
    if context.request_log_enabled {
        context.logger.log(
            "ok",
            format!(
                "request_id={} op={} elapsed_ms={} remote={} detail={} summary={}",
                context.request_id,
                context.operation,
                elapsed.as_millis(),
                context.remote_addr,
                detail.as_ref(),
                context.summary,
            ),
        );
    }
    maybe_log_slow_request(context, elapsed, "completed", detail.as_ref());
}

/// 记录请求失败日志。
/// Record the request-failure log.
fn log_request_failed(context: &RequestLogContext, status: &Status) {
    let elapsed = context.started_at.elapsed();
    context.logger.log(
        "error",
        format!(
            "request_id={} op={} elapsed_ms={} remote={} code={:?} message={} summary={}",
            context.request_id,
            context.operation,
            elapsed.as_millis(),
            context.remote_addr,
            status.code(),
            status.message(),
            context.summary,
        ),
    );
    maybe_log_slow_request(context, elapsed, "failed", status.message());
}

/// 根据阈值记录慢请求日志。
/// Record the slow-request log when the threshold is exceeded.
fn maybe_log_slow_request(
    context: &RequestLogContext,
    elapsed: Duration,
    final_state: &str,
    detail: &str,
) {
    if !context.slow_request_log_enabled || elapsed < context.slow_request_threshold {
        return;
    }

    let summary = if context.include_request_details_in_slow_log {
        context.summary.as_str()
    } else {
        context.operation
    };

    context.logger.log(
        "slow_request",
        format!(
            "request_id={} op={} elapsed_ms={} threshold_ms={} remote={} state={} detail={} summary={}",
            context.request_id,
            context.operation,
            elapsed.as_millis(),
            context.slow_request_threshold.as_millis(),
            context.remote_addr,
            final_state,
            detail,
            summary,
        ),
    );
}

/// 预览文本，压缩空白并按字符截断。
/// Preview text by compacting whitespace and truncating by character count.
fn preview_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "<empty>".to_string();
    }

    let mut preview = String::new();
    for (index, ch) in normalized.chars().enumerate() {
        if index >= max_chars {
            preview.push_str("...");
            return preview;
        }
        preview.push(ch);
    }

    preview
}

#[cfg(test)]
mod tests {
    use super::preview_text;

    #[test]
    fn preview_text_compacts_whitespace_and_truncates() {
        let preview = preview_text("table = demo\nfilter = id   > 1", 160);
        assert_eq!(preview, "table = demo filter = id > 1");

        let preview = preview_text(&format!("prefix {}", "x".repeat(300)), 24);
        assert!(preview.ends_with("..."));
        assert!(preview.len() >= 24);
    }

    #[test]
    fn preview_text_marks_empty_input() {
        assert_eq!(preview_text(" \n\t ", 64), "<empty>");
    }
}
