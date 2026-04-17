/// 列类型定义，描述纯 Rust 库层支持的字段类型。
/// Column type definition describing field types supported by the pure Rust library layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanceDbColumnType {
    Unspecified,
    String,
    Int64,
    Float64,
    Bool,
    VectorFloat32,
    Float32,
    Uint64,
    Int32,
    Uint32,
}

/// 列定义，描述建表时的列结构。
/// Column definition describing the schema of one column during table creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanceDbColumnDef {
    pub name: String,
    pub column_type: LanceDbColumnType,
    pub vector_dim: u32,
    pub nullable: bool,
}

/// 输入格式定义，描述库层接受的数据载荷格式。
/// Input format definition describing payload formats accepted by the library layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanceDbInputFormat {
    Unspecified,
    JsonRows,
    ArrowIpc,
}

/// 输出格式定义，描述检索结果的编码方式。
/// Output format definition describing how search results should be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanceDbOutputFormat {
    Unspecified,
    ArrowIpc,
    JsonRows,
}

impl LanceDbOutputFormat {
    /// 返回输出格式对应的稳定名称。
    /// Return the stable name corresponding to the output format.
    pub fn as_wire_name(&self) -> &'static str {
        match self {
            Self::Unspecified | Self::ArrowIpc => "arrow_ipc",
            Self::JsonRows => "json",
        }
    }
}

/// 建表输入。
/// Create-table input.
#[derive(Debug, Clone)]
pub struct LanceDbCreateTableInput {
    pub table_name: String,
    pub columns: Vec<LanceDbColumnDef>,
    pub overwrite_if_exists: bool,
}

/// 建表结果。
/// Create-table result.
#[derive(Debug, Clone)]
pub struct LanceDbCreateTableResult {
    pub message: String,
}

/// 写入输入。
/// Upsert input.
#[derive(Debug, Clone)]
pub struct LanceDbUpsertInput {
    pub table_name: String,
    pub input_format: LanceDbInputFormat,
    pub data: Vec<u8>,
    pub key_columns: Vec<String>,
}

/// 写入结果。
/// Upsert result.
#[derive(Debug, Clone)]
pub struct LanceDbUpsertResult {
    pub message: String,
    pub version: u64,
    pub input_rows: u64,
    pub inserted_rows: u64,
    pub updated_rows: u64,
    pub deleted_rows: u64,
}

/// 检索输入。
/// Search input.
#[derive(Debug, Clone)]
pub struct LanceDbSearchInput {
    pub table_name: String,
    pub vector: Vec<f32>,
    pub limit: u32,
    pub filter: String,
    pub vector_column: String,
    pub output_format: LanceDbOutputFormat,
}

/// 检索结果。
/// Search result.
#[derive(Debug, Clone)]
pub struct LanceDbSearchResult {
    pub message: String,
    pub format: LanceDbOutputFormat,
    pub rows: u64,
    pub data: Vec<u8>,
}

/// 删除输入。
/// Delete input.
#[derive(Debug, Clone)]
pub struct LanceDbDeleteInput {
    pub table_name: String,
    pub condition: String,
}

/// 删除结果。
/// Delete result.
#[derive(Debug, Clone)]
pub struct LanceDbDeleteResult {
    pub message: String,
    pub version: u64,
    pub deleted_rows: u64,
}

/// 删表输入。
/// Drop-table input.
#[derive(Debug, Clone)]
pub struct LanceDbDropTableInput {
    pub table_name: String,
}

/// 删表结果。
/// Drop-table result.
#[derive(Debug, Clone)]
pub struct LanceDbDropTableResult {
    pub message: String,
}
