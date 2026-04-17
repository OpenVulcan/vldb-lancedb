use crate::engine::{LanceDbEngine, LanceDbEngineOptions};
use crate::manager::DatabaseRuntimeConfig;
use crate::runtime::LanceDbRuntime;
use crate::types::{
    LanceDbColumnDef, LanceDbColumnType, LanceDbCreateTableInput, LanceDbDeleteInput,
    LanceDbDropTableInput, LanceDbInputFormat, LanceDbOutputFormat, LanceDbSearchInput,
    LanceDbUpsertInput,
};
use serde::Deserialize;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;

thread_local! {
    /// 线程局部错误缓冲区，保存最近一次 FFI 调用失败信息。
    /// Thread-local error buffer storing the latest FFI failure message.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// FFI 运行时创建选项。
/// FFI runtime creation options.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VldbLancedbRuntimeOptions {
    pub default_db_path: *const c_char,
    pub db_root: *const c_char,
    pub read_consistency_interval_ms: u64,
    pub has_read_consistency_interval: u8,
    pub max_upsert_payload: usize,
    pub max_search_limit: usize,
    pub max_concurrent_requests: usize,
}

/// FFI 运行时句柄。
/// FFI runtime handle.
pub struct VldbLancedbRuntimeHandle {
    inner: LanceDbRuntime,
}

/// FFI 引擎句柄。
/// FFI engine handle.
#[allow(dead_code)]
pub struct VldbLancedbEngineHandle {
    inner: LanceDbEngine,
}

/// FFI Tokio worker，负责在固定后台线程内串行调度所有异步库调用。
/// FFI Tokio worker responsible for serially dispatching all async library calls on one dedicated background thread.
struct FfiRuntimeWorker {
    sender: mpsc::Sender<FfiRuntimeJob>,
}

/// FFI worker job，封装一次需要在专用 Tokio runtime 中执行的同步任务。
/// FFI worker job encapsulating one synchronous task that must run inside the dedicated Tokio runtime.
type FfiRuntimeJob = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send + 'static>;

/// FFI 字节缓冲区，供调用方读取原始 bytes 结果。
/// FFI byte buffer used by callers to consume raw byte results.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VldbLancedbByteBuffer {
    pub data: *mut u8,
    pub len: usize,
    /// 原始分配容量，仅供本库在释放时恢复 Vec 布局。
    /// Original allocation capacity used only by this library to restore the Vec layout during free.
    pub cap: usize,
}

/// FFI 状态码，供 C / Go 调用方判断调用是否成功。
/// FFI status code used by C / Go callers to determine whether a call succeeded.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VldbLancedbStatusCode {
    Success = 0,
    Failure = 1,
}

/// FFI 输入格式枚举，作为非 JSON 主接口的稳定入参。
/// FFI input-format enum used as stable input for the non-JSON main interface.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VldbLancedbFfiInputFormat {
    Unspecified = 0,
    JsonRows = 1,
    ArrowIpc = 2,
}

/// FFI 输出格式枚举，作为非 JSON 主接口的稳定入参。
/// FFI output-format enum used as stable input for the non-JSON main interface.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VldbLancedbFfiOutputFormat {
    Unspecified = 0,
    ArrowIpc = 1,
    JsonRows = 2,
}

/// FFI 写入结果结构。
/// FFI upsert result structure.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VldbLancedbUpsertResultPod {
    pub version: u64,
    pub input_rows: u64,
    pub inserted_rows: u64,
    pub updated_rows: u64,
    pub deleted_rows: u64,
}

/// FFI 检索结果元信息结构。
/// FFI search-result metadata structure.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VldbLancedbSearchResultMeta {
    pub format: u32,
    pub rows: u64,
    pub byte_length: usize,
}

/// FFI 建表 JSON 输入。
/// FFI create-table JSON input.
#[derive(Debug, Deserialize)]
struct CreateTableJsonInput {
    table_name: String,
    columns: Vec<CreateTableJsonColumn>,
    #[serde(default)]
    overwrite_if_exists: bool,
}

/// FFI 建表列 JSON 输入。
/// FFI create-table column JSON input.
#[derive(Debug, Deserialize)]
struct CreateTableJsonColumn {
    name: String,
    column_type: String,
    #[serde(default)]
    vector_dim: u32,
    #[serde(default = "default_nullable")]
    nullable: bool,
}

/// FFI 写入 JSON 输入。
/// FFI upsert JSON input.
#[derive(Debug, Deserialize)]
struct UpsertJsonInput {
    table_name: String,
    input_format: String,
    #[serde(default)]
    key_columns: Vec<String>,
}

/// FFI 检索 JSON 输入。
/// FFI search JSON input.
#[derive(Debug, Deserialize)]
struct SearchJsonInput {
    table_name: String,
    vector: Vec<f32>,
    #[serde(default = "default_search_limit")]
    limit: u32,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    vector_column: String,
    #[serde(default)]
    output_format: String,
}

/// FFI 删除 JSON 输入。
/// FFI delete JSON input.
#[derive(Debug, Deserialize)]
struct DeleteJsonInput {
    table_name: String,
    condition: String,
}

/// FFI 删表 JSON 输入。
/// FFI drop-table JSON input.
#[derive(Debug, Deserialize)]
struct DropTableJsonInput {
    table_name: String,
}

/// 返回运行时选项的默认值模板。
/// Return the default-value template for runtime options.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_options_default() -> VldbLancedbRuntimeOptions {
    let defaults = LanceDbEngineOptions::default();
    VldbLancedbRuntimeOptions {
        default_db_path: ptr::null(),
        db_root: ptr::null(),
        read_consistency_interval_ms: 0,
        has_read_consistency_interval: 1,
        max_upsert_payload: defaults.max_upsert_payload,
        max_search_limit: defaults.max_search_limit,
        max_concurrent_requests: defaults.max_concurrent_requests,
    }
}

/// 创建嵌入式 LanceDB 运行时。
/// Create the embedded LanceDB runtime.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_create(
    options: VldbLancedbRuntimeOptions,
) -> *mut VldbLancedbRuntimeHandle {
    clear_last_error();

    match build_runtime_from_options(options) {
        Ok(runtime) => Box::into_raw(Box::new(VldbLancedbRuntimeHandle { inner: runtime })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 释放嵌入式 LanceDB 运行时。
/// Destroy the embedded LanceDB runtime.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_destroy(handle: *mut VldbLancedbRuntimeHandle) {
    if handle.is_null() {
        return;
    }

    // SAFETY: `handle` was allocated by `Box::into_raw` in `vldb_lancedb_runtime_create`,
    // and this function consumes it at most once when the caller passes the original pointer.
    unsafe {
        drop(Box::from_raw(handle));
    }
}

/// 打开默认数据库引擎。
/// Open the default database engine.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_open_default_engine(
    handle: *mut VldbLancedbRuntimeHandle,
) -> *mut VldbLancedbEngineHandle {
    clear_last_error();

    let Some(runtime) = runtime_handle_ref(handle) else {
        return ptr::null_mut();
    };
    let runtime = runtime.inner.clone();

    match ffi_block_on(move || async move { runtime.open_default_engine().await }) {
        Ok(engine) => Box::into_raw(Box::new(VldbLancedbEngineHandle { inner: engine })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 按名称打开数据库引擎。
/// Open a database engine by name.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_open_named_engine(
    handle: *mut VldbLancedbRuntimeHandle,
    database_name: *const c_char,
) -> *mut VldbLancedbEngineHandle {
    clear_last_error();

    let Some(runtime) = runtime_handle_ref(handle) else {
        return ptr::null_mut();
    };

    let database_name = match optional_c_string(database_name, "database_name") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };
    let runtime = runtime.inner.clone();

    match ffi_block_on(
        move || async move { runtime.open_named_engine(database_name.as_deref()).await },
    ) {
        Ok(engine) => Box::into_raw(Box::new(VldbLancedbEngineHandle { inner: engine })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 解析默认库或命名库的目标路径，并返回调用方可释放的字符串。
/// Resolve the target path for the default or named database and return a caller-owned string.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_database_path_for_name(
    handle: *mut VldbLancedbRuntimeHandle,
    database_name: *const c_char,
) -> *mut c_char {
    clear_last_error();

    let Some(runtime) = runtime_handle_ref(handle) else {
        return ptr::null_mut();
    };

    let database_name = match optional_c_string(database_name, "database_name") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };

    match runtime
        .inner
        .database_path_for_name(database_name.as_deref())
    {
        Ok(path) => string_into_raw(path),
        Err(error) => {
            set_last_error(error.to_string());
            ptr::null_mut()
        }
    }
}

/// 基于 JSON 输入执行建表操作，并返回结果 JSON 字符串。
/// Execute create-table from JSON input and return the result JSON string.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_create_table_json(
    handle: *mut VldbLancedbEngineHandle,
    input_json: *const c_char,
) -> *mut c_char {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return ptr::null_mut();
    };

    let input = match parse_json_input::<CreateTableJsonInput>(input_json, "input_json") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };

    let input = match map_create_table_input(input) {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };
    let engine = engine.inner.clone();

    match ffi_block_on(move || async move { engine.create_table(input).await }) {
        Ok(result) => json_string_response(serde_json::json!({
            "success": true,
            "message": result.message,
        })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 基于 JSON 描述和原始载荷执行写入操作，并返回结果 JSON 字符串。
/// Execute upsert from JSON metadata plus raw payload and return the result JSON string.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_vector_upsert(
    handle: *mut VldbLancedbEngineHandle,
    input_json: *const c_char,
    data: *const u8,
    data_len: usize,
) -> *mut c_char {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return ptr::null_mut();
    };

    let input = match parse_json_input::<UpsertJsonInput>(input_json, "input_json") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };

    let input = match map_upsert_input(input, data, data_len) {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };
    let engine = engine.inner.clone();

    match ffi_block_on(move || async move { engine.vector_upsert(input).await }) {
        Ok(result) => json_string_response(serde_json::json!({
            "success": true,
            "message": result.message,
            "version": result.version,
            "input_rows": result.input_rows,
            "inserted_rows": result.inserted_rows,
            "updated_rows": result.updated_rows,
            "deleted_rows": result.deleted_rows,
        })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 基于扁平参数执行向量写入主接口，不使用 JSON 元信息。
/// Execute the main vector-upsert path from flat parameters without JSON metadata.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_vector_upsert_raw(
    handle: *mut VldbLancedbEngineHandle,
    table_name: *const c_char,
    input_format: VldbLancedbFfiInputFormat,
    data: *const u8,
    data_len: usize,
    key_columns: *const *const c_char,
    key_columns_len: usize,
    out_result: *mut VldbLancedbUpsertResultPod,
) -> i32 {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return VldbLancedbStatusCode::Failure as i32;
    };
    if out_result.is_null() {
        set_last_error("out_result must not be null");
        return VldbLancedbStatusCode::Failure as i32;
    }

    let result = (|| -> Result<VldbLancedbUpsertResultPod, String> {
        let table_name = required_c_string(table_name, "table_name")?;
        let key_columns = copy_c_string_array(key_columns, key_columns_len, "key_columns")?;
        let input = LanceDbUpsertInput {
            table_name,
            input_format: parse_ffi_input_format(input_format),
            data: copy_input_bytes(data, data_len)?,
            key_columns,
        };
        let engine = engine.inner.clone();

        let result = ffi_block_on(move || async move { engine.vector_upsert(input).await })?;
        Ok(VldbLancedbUpsertResultPod {
            version: result.version,
            input_rows: result.input_rows,
            inserted_rows: result.inserted_rows,
            updated_rows: result.updated_rows,
            deleted_rows: result.deleted_rows,
        })
    })();

    match result {
        Ok(result) => {
            // SAFETY: `out_result` is checked non-null above and points to writable caller memory.
            unsafe {
                *out_result = result;
            }
            VldbLancedbStatusCode::Success as i32
        }
        Err(message) => {
            set_last_error(message);
            VldbLancedbStatusCode::Failure as i32
        }
    }
}

/// 基于 JSON 输入执行向量检索，并将原始结果写入输出字节缓冲区。
/// Execute vector search from JSON input and write the raw result into the output byte buffer.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_vector_search(
    handle: *mut VldbLancedbEngineHandle,
    input_json: *const c_char,
    output_data: *mut VldbLancedbByteBuffer,
) -> *mut c_char {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return ptr::null_mut();
    };

    if output_data.is_null() {
        set_last_error("output_data must not be null");
        return ptr::null_mut();
    }

    let input = match parse_json_input::<SearchJsonInput>(input_json, "input_json") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };

    let input = match map_search_input(input) {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };
    let engine = engine.inner.clone();

    match ffi_block_on(move || async move { engine.vector_search(input).await }) {
        Ok(result) => {
            let data = allocate_byte_buffer(result.data);
            // SAFETY: `output_data` is checked non-null above and points to caller-owned writable memory.
            unsafe {
                *output_data = data;
            }
            json_string_response(serde_json::json!({
                "success": true,
                "message": result.message,
                "format": result.format.as_wire_name(),
                "rows": result.rows,
                "byte_length": data.len,
            }))
        }
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 基于扁平参数执行向量检索主接口，不使用 JSON 元信息。
/// Execute the main vector-search path from flat parameters without JSON metadata.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_vector_search_f32(
    handle: *mut VldbLancedbEngineHandle,
    table_name: *const c_char,
    vector_data: *const f32,
    vector_len: usize,
    limit: u32,
    filter: *const c_char,
    vector_column: *const c_char,
    output_format: VldbLancedbFfiOutputFormat,
    output_data: *mut VldbLancedbByteBuffer,
    out_result: *mut VldbLancedbSearchResultMeta,
) -> i32 {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return VldbLancedbStatusCode::Failure as i32;
    };
    if output_data.is_null() {
        set_last_error("output_data must not be null");
        return VldbLancedbStatusCode::Failure as i32;
    }
    if out_result.is_null() {
        set_last_error("out_result must not be null");
        return VldbLancedbStatusCode::Failure as i32;
    }

    let result = (|| -> Result<(VldbLancedbByteBuffer, VldbLancedbSearchResultMeta), String> {
        let table_name = required_c_string(table_name, "table_name")?;
        let filter = optional_c_string(filter, "filter")?.unwrap_or_default();
        let vector_column = optional_c_string(vector_column, "vector_column")?.unwrap_or_default();
        let vector = copy_input_f32_slice(vector_data, vector_len, "vector_data")?;
        let search_input = LanceDbSearchInput {
            table_name,
            vector,
            limit,
            filter,
            vector_column,
            output_format: parse_ffi_output_format(output_format),
        };
        let engine = engine.inner.clone();

        let result = ffi_block_on(move || async move { engine.vector_search(search_input).await })?;
        let buffer = allocate_byte_buffer(result.data);
        let meta = VldbLancedbSearchResultMeta {
            format: ffi_output_format_code(result.format),
            rows: result.rows,
            byte_length: buffer.len,
        };
        Ok((buffer, meta))
    })();

    match result {
        Ok((buffer, meta)) => {
            // SAFETY: output pointers are checked non-null above and point to writable caller memory.
            unsafe {
                *output_data = buffer;
                *out_result = meta;
            }
            VldbLancedbStatusCode::Success as i32
        }
        Err(message) => {
            set_last_error(message);
            VldbLancedbStatusCode::Failure as i32
        }
    }
}

/// 基于 JSON 输入执行删除操作，并返回结果 JSON 字符串。
/// Execute delete from JSON input and return the result JSON string.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_delete_json(
    handle: *mut VldbLancedbEngineHandle,
    input_json: *const c_char,
) -> *mut c_char {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return ptr::null_mut();
    };

    let input = match parse_json_input::<DeleteJsonInput>(input_json, "input_json") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };

    let input = LanceDbDeleteInput {
        table_name: input.table_name,
        condition: input.condition,
    };
    let engine = engine.inner.clone();

    match ffi_block_on(move || async move { engine.delete(input).await }) {
        Ok(result) => json_string_response(serde_json::json!({
            "success": true,
            "message": result.message,
            "version": result.version,
            "deleted_rows": result.deleted_rows,
        })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 基于 JSON 输入执行删表操作，并返回结果 JSON 字符串。
/// Execute drop-table from JSON input and return the result JSON string.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_drop_table_json(
    handle: *mut VldbLancedbEngineHandle,
    input_json: *const c_char,
) -> *mut c_char {
    clear_last_error();

    let Some(engine) = engine_handle_ref(handle) else {
        return ptr::null_mut();
    };

    let input = match parse_json_input::<DropTableJsonInput>(input_json, "input_json") {
        Ok(value) => value,
        Err(message) => {
            set_last_error(message);
            return ptr::null_mut();
        }
    };

    let input = LanceDbDropTableInput {
        table_name: input.table_name,
    };
    let engine = engine.inner.clone();

    match ffi_block_on(move || async move { engine.drop_table(input).await }) {
        Ok(result) => json_string_response(serde_json::json!({
            "success": true,
            "message": result.message,
        })),
        Err(message) => {
            set_last_error(message);
            ptr::null_mut()
        }
    }
}

/// 释放引擎句柄。
/// Destroy the engine handle.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_destroy(handle: *mut VldbLancedbEngineHandle) {
    if handle.is_null() {
        return;
    }

    // SAFETY: `handle` was allocated by `Box::into_raw` in this module and is released here exactly once.
    unsafe {
        drop(Box::from_raw(handle));
    }
}

/// 释放由本库分配的字节缓冲区。
/// Free a byte buffer allocated by this library.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_bytes_free(buffer: VldbLancedbByteBuffer) {
    if buffer.data.is_null() || buffer.cap == 0 {
        return;
    }

    // SAFETY: `buffer` must originate from `allocate_byte_buffer`, which leaks a Vec and preserves
    // both the original length and capacity for a matching reconstruction here.
    unsafe {
        drop(Vec::from_raw_parts(buffer.data, buffer.len, buffer.cap));
    }
}

/// 释放由本库分配的字符串。
/// Free a string allocated by this library.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    // SAFETY: `value` must come from `CString::into_raw` inside this library.
    unsafe {
        drop(CString::from_raw(value));
    }
}

/// 返回最近一次 FFI 错误消息。
/// Return the latest FFI error message.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_last_error_message() -> *const c_char {
    LAST_ERROR.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|value| value.as_ptr())
            .unwrap_or(ptr::null())
    })
}

/// 清理最近一次 FFI 错误消息。
/// Clear the latest FFI error message.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_clear_last_error() {
    clear_last_error();
}

/// 返回引擎句柄是否为空。
/// Return whether the engine handle is null.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_engine_is_null(handle: *const VldbLancedbEngineHandle) -> u8 {
    if handle.is_null() { 1 } else { 0 }
}

/// 返回运行时句柄是否为空。
/// Return whether the runtime handle is null.
#[unsafe(no_mangle)]
pub extern "C" fn vldb_lancedb_runtime_is_null(handle: *const VldbLancedbRuntimeHandle) -> u8 {
    if handle.is_null() { 1 } else { 0 }
}

/// 将 FFI 运行时选项转换为内部运行时对象。
/// Convert FFI runtime options into the internal runtime object.
fn build_runtime_from_options(
    options: VldbLancedbRuntimeOptions,
) -> Result<LanceDbRuntime, String> {
    let default_db_path = required_c_string(options.default_db_path, "default_db_path")?;
    let db_root = optional_c_string(options.db_root, "db_root")?;

    let config = DatabaseRuntimeConfig {
        default_db_path,
        db_root,
        read_consistency_interval_ms: if options.has_read_consistency_interval == 0 {
            None
        } else {
            Some(options.read_consistency_interval_ms)
        },
    };

    let defaults = LanceDbEngineOptions::default();
    let engine_options = LanceDbEngineOptions {
        max_upsert_payload: normalize_non_zero(
            options.max_upsert_payload,
            defaults.max_upsert_payload,
        ),
        max_search_limit: normalize_non_zero(options.max_search_limit, defaults.max_search_limit),
        max_concurrent_requests: normalize_non_zero(
            options.max_concurrent_requests,
            defaults.max_concurrent_requests,
        ),
    };

    Ok(LanceDbRuntime::new(config, engine_options))
}

/// 从原始句柄中读取共享运行时引用。
/// Read the shared runtime reference from a raw handle.
fn runtime_handle_ref(
    handle: *mut VldbLancedbRuntimeHandle,
) -> Option<&'static VldbLancedbRuntimeHandle> {
    if handle.is_null() {
        set_last_error("runtime handle must not be null");
        return None;
    }

    // SAFETY: the caller promises `handle` points to a valid runtime allocated by this library
    // and the returned reference is only used transiently within the current FFI call.
    Some(unsafe { &*handle })
}

/// 从原始句柄中读取共享引擎引用。
/// Read the shared engine reference from a raw handle.
fn engine_handle_ref(
    handle: *mut VldbLancedbEngineHandle,
) -> Option<&'static VldbLancedbEngineHandle> {
    if handle.is_null() {
        set_last_error("engine handle must not be null");
        return None;
    }

    // SAFETY: the caller promises `handle` points to a valid engine allocated by this library
    // and the returned reference is used only during the current FFI call.
    Some(unsafe { &*handle })
}

/// 将 JSON 文本解析为指定输入结构。
/// Parse JSON text into the target input structure.
fn parse_json_input<T>(value: *const c_char, field_name: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let text = required_c_string(value, field_name)?;
    serde_json::from_str(&text).map_err(|error| format!("failed to parse {field_name}: {error}"))
}

/// 将建表 JSON 输入映射为引擎输入。
/// Map create-table JSON input into the engine input.
fn map_create_table_input(input: CreateTableJsonInput) -> Result<LanceDbCreateTableInput, String> {
    let columns = input
        .columns
        .into_iter()
        .map(map_create_table_column)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(LanceDbCreateTableInput {
        table_name: input.table_name,
        columns,
        overwrite_if_exists: input.overwrite_if_exists,
    })
}

/// 将建表列 JSON 输入映射为列定义。
/// Map create-table column JSON input into the column definition.
fn map_create_table_column(input: CreateTableJsonColumn) -> Result<LanceDbColumnDef, String> {
    Ok(LanceDbColumnDef {
        name: input.name,
        column_type: parse_column_type(&input.column_type)?,
        vector_dim: input.vector_dim,
        nullable: input.nullable,
    })
}

/// 将写入 JSON 输入映射为引擎输入。
/// Map upsert JSON input into the engine input.
fn map_upsert_input(
    input: UpsertJsonInput,
    data: *const u8,
    data_len: usize,
) -> Result<LanceDbUpsertInput, String> {
    Ok(LanceDbUpsertInput {
        table_name: input.table_name,
        input_format: parse_input_format(&input.input_format)?,
        data: copy_input_bytes(data, data_len)?,
        key_columns: input.key_columns,
    })
}

/// 将检索 JSON 输入映射为引擎输入。
/// Map search JSON input into the engine input.
fn map_search_input(input: SearchJsonInput) -> Result<LanceDbSearchInput, String> {
    Ok(LanceDbSearchInput {
        table_name: input.table_name,
        vector: input.vector,
        limit: input.limit,
        filter: input.filter,
        vector_column: input.vector_column,
        output_format: parse_output_format(&input.output_format)?,
    })
}

/// 解析列类型字符串。
/// Parse the column-type string.
fn parse_column_type(value: &str) -> Result<LanceDbColumnType, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "string" => Ok(LanceDbColumnType::String),
        "int64" => Ok(LanceDbColumnType::Int64),
        "float64" => Ok(LanceDbColumnType::Float64),
        "bool" | "boolean" => Ok(LanceDbColumnType::Bool),
        "vector_float32" | "vector-float32" => Ok(LanceDbColumnType::VectorFloat32),
        "float32" => Ok(LanceDbColumnType::Float32),
        "uint64" => Ok(LanceDbColumnType::Uint64),
        "int32" => Ok(LanceDbColumnType::Int32),
        "uint32" => Ok(LanceDbColumnType::Uint32),
        "unspecified" | "" => Ok(LanceDbColumnType::Unspecified),
        other => Err(format!("unsupported column_type: {other}")),
    }
}

/// 解析输入格式字符串。
/// Parse the input-format string.
fn parse_input_format(value: &str) -> Result<LanceDbInputFormat, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "unspecified" | "json" | "json_rows" | "json-rows" => Ok(LanceDbInputFormat::JsonRows),
        "arrow" | "arrow_ipc" | "arrow-ipc" => Ok(LanceDbInputFormat::ArrowIpc),
        other => Err(format!("unsupported input_format: {other}")),
    }
}

/// 解析 FFI 输入格式枚举。
/// Parse the FFI input-format enum.
fn parse_ffi_input_format(value: VldbLancedbFfiInputFormat) -> LanceDbInputFormat {
    match value {
        VldbLancedbFfiInputFormat::Unspecified => LanceDbInputFormat::Unspecified,
        VldbLancedbFfiInputFormat::JsonRows => LanceDbInputFormat::JsonRows,
        VldbLancedbFfiInputFormat::ArrowIpc => LanceDbInputFormat::ArrowIpc,
    }
}

/// 解析输出格式字符串。
/// Parse the output-format string.
fn parse_output_format(value: &str) -> Result<LanceDbOutputFormat, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "unspecified" | "arrow" | "arrow_ipc" | "arrow-ipc" => {
            Ok(LanceDbOutputFormat::ArrowIpc)
        }
        "json" | "json_rows" | "json-rows" => Ok(LanceDbOutputFormat::JsonRows),
        other => Err(format!("unsupported output_format: {other}")),
    }
}

/// 解析 FFI 输出格式枚举。
/// Parse the FFI output-format enum.
fn parse_ffi_output_format(value: VldbLancedbFfiOutputFormat) -> LanceDbOutputFormat {
    match value {
        VldbLancedbFfiOutputFormat::Unspecified => LanceDbOutputFormat::Unspecified,
        VldbLancedbFfiOutputFormat::ArrowIpc => LanceDbOutputFormat::ArrowIpc,
        VldbLancedbFfiOutputFormat::JsonRows => LanceDbOutputFormat::JsonRows,
    }
}

/// 将库层输出格式映射为稳定的 FFI 编码号。
/// Map a library output format into the stable FFI wire code.
fn ffi_output_format_code(value: LanceDbOutputFormat) -> u32 {
    match value {
        LanceDbOutputFormat::Unspecified => VldbLancedbFfiOutputFormat::Unspecified as u32,
        LanceDbOutputFormat::ArrowIpc => VldbLancedbFfiOutputFormat::ArrowIpc as u32,
        LanceDbOutputFormat::JsonRows => VldbLancedbFfiOutputFormat::JsonRows as u32,
    }
}

/// 复制调用方传入的字节载荷。
/// Copy the byte payload provided by the caller.
fn copy_input_bytes(data: *const u8, data_len: usize) -> Result<Vec<u8>, String> {
    if data_len == 0 {
        return Ok(Vec::new());
    }
    if data.is_null() {
        return Err("data must not be null when data_len > 0".to_string());
    }

    // SAFETY: `data` points to a readable buffer of `data_len` bytes owned by the caller for this FFI call.
    let slice = unsafe { std::slice::from_raw_parts(data, data_len) };
    Ok(slice.to_vec())
}

/// 复制调用方传入的 `f32` 向量切片。
/// Copy an `f32` vector slice provided by the caller.
fn copy_input_f32_slice(
    data: *const f32,
    data_len: usize,
    field_name: &str,
) -> Result<Vec<f32>, String> {
    if data_len == 0 {
        return Err(format!("{field_name} must not be empty"));
    }
    if data.is_null() {
        return Err(format!("{field_name} must not be null when data_len > 0"));
    }

    // SAFETY: `data` points to a readable `f32` buffer of `data_len` elements for this FFI call.
    let slice = unsafe { std::slice::from_raw_parts(data, data_len) };
    Ok(slice.to_vec())
}

/// 复制调用方传入的 C 字符串数组。
/// Copy an array of C strings provided by the caller.
fn copy_c_string_array(
    values: *const *const c_char,
    values_len: usize,
    field_name: &str,
) -> Result<Vec<String>, String> {
    if values_len == 0 {
        return Ok(Vec::new());
    }
    if values.is_null() {
        return Err(format!("{field_name} must not be null when values_len > 0"));
    }

    // SAFETY: `values` points to a readable array of `values_len` pointers for this FFI call.
    let slice = unsafe { std::slice::from_raw_parts(values, values_len) };
    slice
        .iter()
        .enumerate()
        .map(|(index, value)| required_c_string(*value, &format!("{field_name}[{index}]")))
        .collect()
}

/// 将结果字节向量转换为 FFI 字节缓冲区。
/// Convert result bytes into an FFI byte buffer.
fn allocate_byte_buffer(bytes: Vec<u8>) -> VldbLancedbByteBuffer {
    let len = bytes.len();
    if len == 0 {
        return VldbLancedbByteBuffer {
            data: ptr::null_mut(),
            len: 0,
            cap: 0,
        };
    }

    let mut bytes = bytes;
    let cap = bytes.capacity();
    let data = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    VldbLancedbByteBuffer { data, len, cap }
}

/// 将 JSON 值编码成由调用方持有的字符串响应。
/// Encode a JSON value into a caller-owned string response.
fn json_string_response(value: serde_json::Value) -> *mut c_char {
    match serde_json::to_string(&value) {
        Ok(text) => string_into_raw(text),
        Err(error) => {
            set_last_error(format!("failed to serialize JSON response: {error}"));
            ptr::null_mut()
        }
    }
}

/// 返回默认 nullable 配置。
/// Return the default nullable configuration.
fn default_nullable() -> bool {
    true
}

/// 返回默认检索条数。
/// Return the default search limit.
fn default_search_limit() -> u32 {
    10
}

/// 读取必填 C 字符串。
/// Read a required C string.
fn required_c_string(value: *const c_char, field_name: &str) -> Result<String, String> {
    if value.is_null() {
        return Err(format!("{field_name} must not be null"));
    }

    let text = c_string_to_owned(value, field_name)?;
    if text.trim().is_empty() {
        return Err(format!("{field_name} must not be empty"));
    }

    Ok(text)
}

/// 读取可选 C 字符串。
/// Read an optional C string.
fn optional_c_string(value: *const c_char, field_name: &str) -> Result<Option<String>, String> {
    if value.is_null() {
        return Ok(None);
    }

    let text = c_string_to_owned(value, field_name)?;
    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

/// 将原始 C 字符串复制为 Rust String。
/// Copy a raw C string into a Rust String.
fn c_string_to_owned(value: *const c_char, field_name: &str) -> Result<String, String> {
    // SAFETY: `value` is expected to point to a valid NUL-terminated string supplied by the caller.
    let c_str = unsafe { CStr::from_ptr(value) };
    c_str
        .to_str()
        .map(|text| text.to_string())
        .map_err(|_| format!("{field_name} must be valid UTF-8"))
}

/// 将字符串转换为调用方持有的原始指针。
/// Convert a string into a caller-owned raw pointer.
fn string_into_raw(value: String) -> *mut c_char {
    let sanitized = sanitize_message(value);
    match CString::new(sanitized) {
        Ok(text) => text.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// 在零值时回退到默认值。
/// Fall back to the default value when the input is zero.
fn normalize_non_zero(value: usize, default_value: usize) -> usize {
    if value == 0 { default_value } else { value }
}

/// 返回全局 FFI Tokio worker。
/// Return the global FFI Tokio worker.
fn ffi_runtime_worker() -> Result<&'static FfiRuntimeWorker, String> {
    static FFI_RUNTIME_WORKER: OnceLock<Result<FfiRuntimeWorker, String>> = OnceLock::new();

    match FFI_RUNTIME_WORKER.get_or_init(|| {
        let (job_tx, job_rx) = mpsc::channel::<FfiRuntimeJob>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        thread::Builder::new()
            .name("vldb-lancedb-ffi-dispatch".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .thread_name("vldb-lancedb-ffi")
                    .build()
                {
                    Ok(runtime) => {
                        let _ = ready_tx.send(Ok(()));
                        runtime
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("failed to build FFI runtime: {error}")));
                        return;
                    }
                };

                while let Ok(job) = job_rx.recv() {
                    // Keep the worker thread alive even if one specific dispatched job panics,
                    // so later FFI calls can still receive a deterministic error instead of losing the runtime entirely.
                    // 即使单次任务 panic，也尽量保持 worker 线程存活，
                    // 让后续 FFI 调用至少还能拿到确定性错误，而不是把整个 runtime 一起打死。
                    let _ =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&runtime)));
                }
            })
            .map_err(|error| format!("failed to spawn FFI runtime worker: {error}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(FfiRuntimeWorker { sender: job_tx }),
            Ok(Err(message)) => Err(message),
            Err(_) => Err("failed to receive FFI runtime worker startup signal".to_string()),
        }
    }) {
        Ok(worker) => Ok(worker),
        Err(message) => Err(message.clone()),
    }
}

/// 在全局 FFI worker 中执行异步任务。
/// Execute an async task inside the global FFI worker.
fn ffi_block_on<BuildFuture, F, T, E>(build_future: BuildFuture) -> Result<T, String>
where
    BuildFuture: FnOnce() -> F + Send + 'static,
    F: std::future::Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let worker = ffi_runtime_worker()?;
    let (result_tx, result_rx) = mpsc::sync_channel::<Result<T, String>>(1);

    worker
        .sender
        .send(Box::new(move |runtime| {
            let outcome = runtime
                .block_on(build_future())
                .map_err(|error| error.to_string());
            let _ = result_tx.send(outcome);
        }))
        .map_err(|_| "FFI runtime worker has stopped".to_string())?;

    result_rx
        .recv()
        .map_err(|_| "FFI runtime worker dropped the response channel".to_string())?
}

/// 记录最近一次错误。
/// Store the latest error.
fn set_last_error(message: impl Into<String>) {
    let sanitized = sanitize_message(message.into());
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = CString::new(sanitized).ok();
    });
}

/// 清理最近一次错误。
/// Clear the latest error.
fn clear_last_error() {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// 清理不允许出现在 C 字符串中的 NUL 字符。
/// Sanitize NUL characters that are not allowed inside C strings.
fn sanitize_message(message: impl Into<String>) -> String {
    message.into().replace('\0', " ")
}

#[cfg(test)]
mod tests {
    use super::{
        CreateTableJsonInput, SearchJsonInput, UpsertJsonInput, VldbLancedbRuntimeOptions,
        allocate_byte_buffer, build_runtime_from_options, default_search_limit, ffi_block_on,
        map_create_table_input, map_search_input, map_upsert_input, normalize_non_zero,
        parse_column_type, parse_input_format, parse_output_format, sanitize_message,
        vldb_lancedb_bytes_free, vldb_lancedb_runtime_options_default,
    };
    use std::ffi::CString;
    use std::thread;

    #[test]
    fn runtime_options_default_exposes_engine_defaults() {
        let options = vldb_lancedb_runtime_options_default();
        assert!(options.default_db_path.is_null());
        assert_eq!(options.has_read_consistency_interval, 1);
        assert!(options.max_upsert_payload > 0);
        assert!(options.max_search_limit > 0);
        assert!(options.max_concurrent_requests > 0);
    }

    #[test]
    fn normalize_non_zero_uses_default_for_zero() {
        assert_eq!(normalize_non_zero(0, 42), 42);
        assert_eq!(normalize_non_zero(7, 42), 7);
    }

    #[test]
    fn sanitize_message_replaces_nul_bytes() {
        assert_eq!(sanitize_message("a\0b"), "a b");
    }

    #[test]
    fn allocate_byte_buffer_preserves_capacity_for_free() {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(b"demo");

        let buffer = allocate_byte_buffer(bytes);
        assert_eq!(buffer.len, 4);
        assert_eq!(buffer.cap, 16);

        vldb_lancedb_bytes_free(buffer);
    }

    #[test]
    fn runtime_can_be_built_from_ffi_options() {
        let default_db_path = CString::new("/tmp/vldb-default").expect("cstring should build");
        let db_root = CString::new("/tmp/vldb-root").expect("cstring should build");
        let options = VldbLancedbRuntimeOptions {
            default_db_path: default_db_path.as_ptr(),
            db_root: db_root.as_ptr(),
            read_consistency_interval_ms: 250,
            has_read_consistency_interval: 1,
            max_upsert_payload: 1024,
            max_search_limit: 64,
            max_concurrent_requests: 8,
        };

        let runtime = build_runtime_from_options(options).expect("runtime should build");
        assert_eq!(runtime.engine_options().max_upsert_payload, 1024);
        assert_eq!(
            runtime
                .database_path_for_name(Some("memory"))
                .expect("named path should resolve"),
            std::path::PathBuf::from("/tmp/vldb-root")
                .join("memory")
                .to_string_lossy()
                .to_string()
        );
    }

    #[test]
    fn parse_helpers_support_expected_wire_names() {
        assert_eq!(
            parse_column_type("vector_float32").expect("column type should parse") as u8,
            crate::types::LanceDbColumnType::VectorFloat32 as u8
        );
        assert_eq!(
            parse_input_format("arrow_ipc").expect("input format should parse") as u8,
            crate::types::LanceDbInputFormat::ArrowIpc as u8
        );
        assert_eq!(
            parse_output_format("json").expect("output format should parse") as u8,
            crate::types::LanceDbOutputFormat::JsonRows as u8
        );
        assert_eq!(default_search_limit(), 10);
    }

    #[test]
    fn json_mapping_builds_engine_inputs() {
        let create_input = map_create_table_input(CreateTableJsonInput {
            table_name: "demo".to_string(),
            columns: vec![super::CreateTableJsonColumn {
                name: "id".to_string(),
                column_type: "string".to_string(),
                vector_dim: 0,
                nullable: false,
            }],
            overwrite_if_exists: true,
        })
        .expect("create input should map");
        assert_eq!(create_input.table_name, "demo");
        assert_eq!(create_input.columns.len(), 1);
        assert!(create_input.overwrite_if_exists);

        let payload = b"[{\"id\":\"1\"}]";
        let upsert_input = map_upsert_input(
            UpsertJsonInput {
                table_name: "demo".to_string(),
                input_format: "json".to_string(),
                key_columns: vec!["id".to_string()],
            },
            payload.as_ptr(),
            payload.len(),
        )
        .expect("upsert input should map");
        assert_eq!(upsert_input.table_name, "demo");
        assert_eq!(upsert_input.data, payload);
        assert_eq!(upsert_input.key_columns, vec!["id".to_string()]);

        let search_input = map_search_input(SearchJsonInput {
            table_name: "demo".to_string(),
            vector: vec![0.1, 0.2],
            limit: 3,
            filter: "id = '1'".to_string(),
            vector_column: "embedding".to_string(),
            output_format: "json".to_string(),
        })
        .expect("search input should map");
        assert_eq!(search_input.limit, 3);
        assert_eq!(search_input.vector_column, "embedding");
    }

    #[test]
    fn ffi_block_on_dispatches_cross_thread_jobs_without_enter_guard_panics() {
        let mut handles = Vec::new();
        for thread_index in 0..6usize {
            handles.push(thread::spawn(move || {
                for call_index in 0..12usize {
                    let expected = thread_index * 100 + call_index;
                    let value = ffi_block_on(move || async move {
                        let value = tokio::task::spawn_blocking(move || expected)
                            .await
                            .map_err(|error| std::io::Error::other(error.to_string()))?;
                        Ok::<usize, std::io::Error>(value)
                    })
                    .expect("ffi worker should execute the dispatched future");
                    assert_eq!(value, expected);
                }
            }));
        }

        for handle in handles {
            handle
                .join()
                .expect("cross-thread ffi caller should finish");
        }
    }
}
