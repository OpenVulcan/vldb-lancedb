#ifndef VLDB_LANCEDB_H
#define VLDB_LANCEDB_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * 嵌入式 LanceDB 运行时创建参数。
 * Embedded LanceDB runtime creation options.
 */
typedef struct VldbLancedbRuntimeOptions {
    const char* default_db_path;
    const char* db_root;
    uint64_t read_consistency_interval_ms;
    uint8_t has_read_consistency_interval;
    size_t max_upsert_payload;
    size_t max_search_limit;
    size_t max_concurrent_requests;
} VldbLancedbRuntimeOptions;

/*
 * 运行时句柄前置声明。
 * Forward declaration of the runtime handle.
 */
typedef struct VldbLancedbRuntimeHandle VldbLancedbRuntimeHandle;

/*
 * 引擎句柄前置声明。
 * Forward declaration of the engine handle.
 */
typedef struct VldbLancedbEngineHandle VldbLancedbEngineHandle;

/*
 * 原始字节缓冲区。
 * Raw byte buffer.
 */
typedef struct VldbLancedbByteBuffer {
    uint8_t* data;
    size_t len;
    size_t cap;
} VldbLancedbByteBuffer;

/*
 * FFI 状态码；0 表示成功，1 表示失败。
 * FFI status code; 0 means success and 1 means failure.
 */
typedef enum VldbLancedbStatusCode {
    VLDB_LANCEDB_STATUS_SUCCESS = 0,
    VLDB_LANCEDB_STATUS_FAILURE = 1
} VldbLancedbStatusCode;

/*
 * FFI 输入格式枚举，供非 JSON 主接口使用。
 * FFI input-format enum used by the non-JSON main interface.
 */
typedef enum VldbLancedbFfiInputFormat {
    VLDB_LANCEDB_INPUT_FORMAT_UNSPECIFIED = 0,
    VLDB_LANCEDB_INPUT_FORMAT_JSON_ROWS = 1,
    VLDB_LANCEDB_INPUT_FORMAT_ARROW_IPC = 2
} VldbLancedbFfiInputFormat;

/*
 * FFI 输出格式枚举，供非 JSON 主接口使用。
 * FFI output-format enum used by the non-JSON main interface.
 */
typedef enum VldbLancedbFfiOutputFormat {
    VLDB_LANCEDB_OUTPUT_FORMAT_UNSPECIFIED = 0,
    VLDB_LANCEDB_OUTPUT_FORMAT_ARROW_IPC = 1,
    VLDB_LANCEDB_OUTPUT_FORMAT_JSON_ROWS = 2
} VldbLancedbFfiOutputFormat;

/*
 * 非 JSON 写入主接口的结果结构。
 * Result structure for the non-JSON main upsert interface.
 */
typedef struct VldbLancedbUpsertResultPod {
    uint64_t version;
    uint64_t input_rows;
    uint64_t inserted_rows;
    uint64_t updated_rows;
    uint64_t deleted_rows;
} VldbLancedbUpsertResultPod;

/*
 * 非 JSON 检索主接口的结果元信息结构。
 * Search-result metadata structure for the non-JSON main search interface.
 */
typedef struct VldbLancedbSearchResultMeta {
    uint32_t format;
    uint64_t rows;
    size_t byte_length;
} VldbLancedbSearchResultMeta;

/*
 * 返回运行时选项的默认值模板。
 * Return the default-value template for runtime options.
 */
VldbLancedbRuntimeOptions vldb_lancedb_runtime_options_default(void);

/*
 * 创建嵌入式 LanceDB 运行时。
 * Create the embedded LanceDB runtime.
 */
VldbLancedbRuntimeHandle* vldb_lancedb_runtime_create(VldbLancedbRuntimeOptions options);

/*
 * 释放嵌入式 LanceDB 运行时。
 * Destroy the embedded LanceDB runtime.
 */
void vldb_lancedb_runtime_destroy(VldbLancedbRuntimeHandle* handle);

/*
 * 打开默认数据库引擎。
 * Open the default database engine.
 */
VldbLancedbEngineHandle* vldb_lancedb_runtime_open_default_engine(
    VldbLancedbRuntimeHandle* handle
);

/*
 * 按名称打开数据库引擎；传入 NULL 或空字符串时将回退到默认库。
 * Open a database engine by name; passing NULL or an empty string falls back to the default database.
 */
VldbLancedbEngineHandle* vldb_lancedb_runtime_open_named_engine(
    VldbLancedbRuntimeHandle* handle,
    const char* database_name
);

/*
 * 解析默认库或命名库的目标路径，返回值需使用 vldb_lancedb_string_free 释放。
 * Resolve the target path for the default or named database; the returned string must be freed
 * with vldb_lancedb_string_free.
 */
char* vldb_lancedb_runtime_database_path_for_name(
    VldbLancedbRuntimeHandle* handle,
    const char* database_name
);

/*
 * 基于 JSON 输入执行建表操作，返回结果 JSON 字符串。
 * Execute create-table from JSON input and return the result JSON string.
 */
char* vldb_lancedb_engine_create_table_json(
    VldbLancedbEngineHandle* handle,
    const char* input_json
);

/*
 * 基于 JSON 元信息和原始载荷执行写入操作，返回结果 JSON 字符串。
 * Execute upsert from JSON metadata plus raw payload and return the result JSON string.
 */
char* vldb_lancedb_engine_vector_upsert(
    VldbLancedbEngineHandle* handle,
    const char* input_json,
    const uint8_t* data,
    size_t data_len
);

/*
 * 基于扁平参数执行向量写入主接口，不使用 JSON 元信息。
 * Execute the main vector-upsert interface from flat parameters without JSON metadata.
 */
int32_t vldb_lancedb_engine_vector_upsert_raw(
    VldbLancedbEngineHandle* handle,
    const char* table_name,
    VldbLancedbFfiInputFormat input_format,
    const uint8_t* data,
    size_t data_len,
    const char* const* key_columns,
    size_t key_columns_len,
    VldbLancedbUpsertResultPod* out_result
);

/*
 * 基于 JSON 输入执行向量检索，并将原始结果写入输出字节缓冲区。
 * Execute vector search from JSON input and write the raw result into the output byte buffer.
 */
char* vldb_lancedb_engine_vector_search(
    VldbLancedbEngineHandle* handle,
    const char* input_json,
    VldbLancedbByteBuffer* output_data
);

/*
 * 基于扁平参数执行向量检索主接口，不使用 JSON 元信息。
 * Execute the main vector-search interface from flat parameters without JSON metadata.
 */
int32_t vldb_lancedb_engine_vector_search_f32(
    VldbLancedbEngineHandle* handle,
    const char* table_name,
    const float* vector_data,
    size_t vector_len,
    uint32_t limit,
    const char* filter,
    const char* vector_column,
    VldbLancedbFfiOutputFormat output_format,
    VldbLancedbByteBuffer* output_data,
    VldbLancedbSearchResultMeta* out_result
);

/*
 * 基于 JSON 输入执行删除操作，返回结果 JSON 字符串。
 * Execute delete from JSON input and return the result JSON string.
 */
char* vldb_lancedb_engine_delete_json(
    VldbLancedbEngineHandle* handle,
    const char* input_json
);

/*
 * 基于 JSON 输入执行删表操作，返回结果 JSON 字符串。
 * Execute drop-table from JSON input and return the result JSON string.
 */
char* vldb_lancedb_engine_drop_table_json(
    VldbLancedbEngineHandle* handle,
    const char* input_json
);

/*
 * 释放引擎句柄。
 * Destroy the engine handle.
 */
void vldb_lancedb_engine_destroy(VldbLancedbEngineHandle* handle);

/*
 * 释放由本库分配的字节缓冲区。
 * Free a byte buffer allocated by this library.
 */
void vldb_lancedb_bytes_free(VldbLancedbByteBuffer buffer);

/*
 * 释放由本库分配的字符串。
 * Free a string allocated by this library.
 */
void vldb_lancedb_string_free(char* value);

/*
 * 返回最近一次 FFI 错误消息；返回指针在下一次错误更新或 clear 后失效。
 * Return the latest FFI error message; the pointer becomes invalid after the next error update or clear.
 */
const char* vldb_lancedb_last_error_message(void);

/*
 * 清理最近一次 FFI 错误消息。
 * Clear the latest FFI error message.
 */
void vldb_lancedb_clear_last_error(void);

/*
 * 返回引擎句柄是否为空。
 * Return whether the engine handle is null.
 */
uint8_t vldb_lancedb_engine_is_null(const VldbLancedbEngineHandle* handle);

/*
 * 返回运行时句柄是否为空。
 * Return whether the runtime handle is null.
 */
uint8_t vldb_lancedb_runtime_is_null(const VldbLancedbRuntimeHandle* handle);

#ifdef __cplusplus
}
#endif

#endif /* VLDB_LANCEDB_H */
