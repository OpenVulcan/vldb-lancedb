# vldb-lancedb 库模式与 FFI 接入说明

## 1. 文档目标

本文专门说明 `vldb-lancedb` 在**库模式**下的使用方式，覆盖两类接入路径：

- **Rust 直接嵌入**
  - 通过 `rlib` 方式复用 `runtime / manager / engine / types`
- **动态库 + FFI**
  - 通过 `cdylib` 产出的动态库与 `include/vldb_lancedb.h` 头文件进行接入

当前推荐分层如下：

- **Rust / MCP**
  - 直接使用 `lib` 的 typed Rust API
  - 不建议再经过 JSON FFI 绕行
- **Go / 其他原生宿主**
  - 优先使用 FFI 的非 JSON 主接口
  - 高频向量写入与检索不建议继续走 JSON
- **JSON FFI**
  - 保留为兼容层
  - 主要给 Lua / Python / 调试 / 其他潜在项目预留

这份文档不讨论 gRPC 服务部署、配置文件发现或 Docker 运行细节；这些内容请继续参考：

- [README](../README.md)
- [中文完整指南](./README.zh-CN.md)

---

## 2. 库模式的边界

`vldb-lancedb` 当前同时支持三种入口：

1. **bin**
   - 直接启动 gRPC 服务
   - 读取 JSON 配置文件
   - 绑定一个默认数据库

2. **lib**
   - 以纯 Rust API 的方式暴露核心 LanceDB 能力
   - 不依赖 gRPC request/response
   - 不依赖配置文件发现逻辑

3. **ffi**
   - 以稳定 C ABI 暴露运行时与引擎能力
   - 供非 Rust 宿主加载动态库

库模式的设计原则是：

- **不依赖配置文件**
- **不依赖 gRPC**
- **调用方自己决定运行时参数**
- **调用方自己管理日志策略、实例生命周期与上层业务流程**

换句话说，库模式只负责：

- 打开数据库
- 创建引擎
- 建表
- 写入
- 检索
- 删除
- 删表

而不负责：

- 监听端口
- 解析 `--config`
- 自动启动 gRPC server
- 固定日志目录或固定宿主行为

---

## 3. 构建产物

### 3.1 Rust 库产物

`Cargo.toml` 当前声明：

```toml
[lib]
name = "vldb_lancedb"
crate-type = ["rlib", "cdylib"]
```

因此构建后会同时得到：

- `rlib`
  - 给 Rust 工程直接依赖
- `cdylib`
  - 给 C / C++ / Lua FFI / 其他原生宿主加载

### 3.2 动态库文件

常见 release 产物路径如下：

- Windows
  - `target/release/vldb_lancedb.dll`
- Linux
  - `target/release/libvldb_lancedb.so`
- macOS
  - `target/release/libvldb_lancedb.dylib`

### 3.3 头文件

FFI 对外头文件固定为：

- `include/vldb_lancedb.h`

动态库调用方应始终以该头文件为准，不应自行猜测导出函数签名。

---

## 4. Rust 直接嵌入

### 4.1 主要模块

当前 `lib.rs` 暴露：

- `manager`
- `types`
- `engine`
- `runtime`
- `ffi`

其中最适合上层直接使用的是：

- `runtime::LanceDbRuntime`
- `engine::LanceDbEngine`
- `types::*`

### 4.2 推荐入口

推荐从 `LanceDbRuntime` 开始，而不是上层自己手动拼 `DatabaseManager + LanceDbEngineOptions + LanceDbEngine`。

典型流程：

1. 构造 `DatabaseRuntimeConfig`
2. 构造 `LanceDbEngineOptions`
3. 创建 `LanceDbRuntime`
4. 打开默认库或命名库
5. 获取 `LanceDbEngine`
6. 调用引擎方法

### 4.3 典型 Rust 调用流程

```rust
use vldb_lancedb::engine::LanceDbEngineOptions;
use vldb_lancedb::manager::DatabaseRuntimeConfig;
use vldb_lancedb::runtime::LanceDbRuntime;

let runtime = LanceDbRuntime::new(
    DatabaseRuntimeConfig {
        default_db_path: "./data/default".to_string(),
        db_root: Some("./data/databases".to_string()),
        read_consistency_interval_ms: Some(0),
    },
    LanceDbEngineOptions::default(),
);

let engine = runtime.open_default_engine().await?;
```

如果需要多库：

```rust
let engine = runtime.open_named_engine(Some("memory")).await?;
```

---

## 5. FFI 模式概览

### 5.1 FFI 设计原则

FFI 层当前区分两类接口：

- **非 JSON 主接口**
  - 供 Go / 原生宿主优先使用
  - 通过扁平参数、枚举和结果结构体承载高频向量路径
- **JSON 兼容接口**
  - 继续保留
  - 供脚本语言和调试快速接入

兼容层当前采用两类数据边界：

- **JSON 字符串**
  - 用于承载元信息输入与结果摘要
- **原始 bytes**
  - 用于承载 upsert 的数据载荷
  - 用于承载 search 的原始结果数据

兼容层这样做的好处是：

- ABI 简洁
- 调用方不需要理解复杂 Rust 结构体
- 便于 Lua / MCP / manager / 本地宿主桥接

而主接口的目标是：

- 高频路径不走 JSON
- Go 调用时不必承担向量的 JSON 编解码损耗
- 仍然保持 C ABI 稳定

### 5.2 主要句柄

头文件中有两个前置声明句柄：

- `VldbLancedbRuntimeHandle`
- `VldbLancedbEngineHandle`

职责划分：

- `Runtime`
  - 管理数据库根路径与实例打开逻辑
- `Engine`
  - 绑定具体数据库并执行表操作

### 5.3 原始结果缓冲区

向量检索会用到：

- `VldbLancedbByteBuffer`

定义：

```c
typedef struct VldbLancedbByteBuffer {
    uint8_t* data;
    size_t len;
    size_t cap;
} VldbLancedbByteBuffer;
```

调用方必须使用：

- `vldb_lancedb_bytes_free`

释放这块内存。

### 5.4 FFI runtime 执行模型与兼容性

从 2026-04 的修复开始，FFI 层不再让外部宿主线程直接在全局 Tokio runtime 上反复 `block_on`。

当前实现改为：

- 一个固定后台 worker 线程
- 一个由该线程独占的 `current_thread` Tokio runtime
- 所有 FFI async 调用通过 channel 投递给该 worker 执行

这样调整的直接原因，是为了修复某些集成场景下可能出现的：

- `EnterGuard values dropped out of order`

这类问题主要出现在：

- Go + purego
- 多线程宿主
- 同进程反复初始化 / 关闭 runtime / engine

兼容性结论如下：

- **ABI 不变**
  - 已有导出符号未变
  - `include/vldb_lancedb.h` 不需要调用方重写绑定
- **调用方式不变**
  - 现有 `Runtime -> Engine -> 操作 -> Destroy` 顺序继续有效
- **gRPC 模式不受影响**
  - 本次调整只作用于 `ffi.rs`
  - 不改变 `main.rs` 的服务模式运行时
- **Rust 直接嵌入不受影响**
  - 直接调用 `runtime::LanceDbRuntime` / `engine::LanceDbEngine` 的 typed Rust API 不经过这层 FFI worker

需要如实说明的一点是：

- FFI 入口现在会在 worker 线程上串行调度
- 因此对于“多个宿主线程同时大量发起 FFI 调用”的极端场景，入口层并行度会低于旧实现
- 但当前多数本地嵌入宿主更关注稳定性、生命周期一致性和跨语言可预测行为，因此这个折中是有意选择

如果后续需要在 **保持稳定性** 的前提下进一步恢复更高 FFI 并发度，应在专用 worker 模型之上继续演进，而不是回退到“宿主线程直接 `block_on` 全局 runtime”的旧做法。

---

## 6. Runtime 相关接口

### 6.1 默认选项模板

```c
VldbLancedbRuntimeOptions vldb_lancedb_runtime_options_default(void);
```

建议调用方总是先取默认模板，再覆盖自己关心的字段。

关键字段：

- `default_db_path`
  - 默认数据库路径
- `db_root`
  - 命名库根目录
- `read_consistency_interval_ms`
  - 跨进程刷新间隔
- `has_read_consistency_interval`
  - 是否启用该字段
- `max_upsert_payload`
  - 单次 upsert payload 上限
- `max_search_limit`
  - 单次 search limit 上限
- `max_concurrent_requests`
  - 引擎内部并发限制

### 6.2 创建与释放 Runtime

```c
VldbLancedbRuntimeHandle* vldb_lancedb_runtime_create(VldbLancedbRuntimeOptions options);
void vldb_lancedb_runtime_destroy(VldbLancedbRuntimeHandle* handle);
```

规则：

- 创建失败时返回 `NULL`
- 失败原因通过 `vldb_lancedb_last_error_message()` 读取
- 成功创建后，最终必须调用 `vldb_lancedb_runtime_destroy()`

### 6.3 打开数据库引擎

```c
VldbLancedbEngineHandle* vldb_lancedb_runtime_open_default_engine(
    VldbLancedbRuntimeHandle* handle
);

VldbLancedbEngineHandle* vldb_lancedb_runtime_open_named_engine(
    VldbLancedbRuntimeHandle* handle,
    const char* database_name
);
```

规则：

- 默认库：使用 `default_db_path`
- 命名库：使用 `db_root/<database_name>`
- `database_name` 传 `NULL` 或空字符串时，回退到默认库
- `database_name` 必须是单层名称，不能包含 `/`、`\`、绝对路径或 `..` 之类的路径逃逸片段

### 6.4 查询数据库目标路径

```c
char* vldb_lancedb_runtime_database_path_for_name(
    VldbLancedbRuntimeHandle* handle,
    const char* database_name
);
```

返回值需要由调用方使用：

- `vldb_lancedb_string_free`

释放。

---

## 7. Engine 相关接口

### 7.1 建表

```c
char* vldb_lancedb_engine_create_table_json(
    VldbLancedbEngineHandle* handle,
    const char* input_json
);
```

输入 JSON 结构：

```json
{
  "table_name": "memory",
  "columns": [
    {
      "name": "id",
      "column_type": "string",
      "nullable": false
    },
    {
      "name": "embedding",
      "column_type": "vector_float32",
      "vector_dim": 1536,
      "nullable": false
    }
  ],
  "overwrite_if_exists": false
}
```

`column_type` 当前支持：

- `string`
- `int32`
- `int64`
- `uint32`
- `uint64`
- `float32`
- `float64`
- `bool`
- `vector_float32`

返回 JSON 示例：

```json
{
  "success": true,
  "message": "table created"
}
```

### 7.2 向量写入

```c
char* vldb_lancedb_engine_vector_upsert(
    VldbLancedbEngineHandle* handle,
    const char* input_json,
    const uint8_t* data,
    size_t data_len
);
```

输入 JSON 元信息：

```json
{
  "table_name": "memory",
  "input_format": "json",
  "key_columns": ["id"]
}
```

`input_format` 当前支持：

- `json`
- `json_rows`
- `arrow`
- `arrow_ipc`

`data` 则是真实载荷，例如：

- JSON Rows 字节串
- Arrow IPC 二进制

返回 JSON 示例：

```json
{
  "success": true,
  "message": "upsert completed",
  "version": 3,
  "input_rows": 10,
  "inserted_rows": 10,
  "updated_rows": 0,
  "deleted_rows": 0
}
```

### 7.2.1 非 JSON 主写入接口

```c
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
```

说明：

- 这是当前推荐给 Go / 原生宿主的主写入接口
- `table_name`、`input_format`、`key_columns` 直接走扁平参数
- `data + data_len` 直接承载原始载荷
- `out_result` 返回结构化统计结果

成功时：

- 返回 `VLDB_LANCEDB_STATUS_SUCCESS`
- `out_result` 中可直接读取：
  - `version`
  - `input_rows`
  - `inserted_rows`
  - `updated_rows`
  - `deleted_rows`

失败时：

- 返回 `VLDB_LANCEDB_STATUS_FAILURE`
- 通过 `vldb_lancedb_last_error_message()` 读取错误信息

### 7.3 向量检索

```c
char* vldb_lancedb_engine_vector_search(
    VldbLancedbEngineHandle* handle,
    const char* input_json,
    VldbLancedbByteBuffer* output_data
);
```

输入 JSON：

```json
{
  "table_name": "memory",
  "vector": [0.1, 0.2, 0.3],
  "limit": 10,
  "filter": "",
  "vector_column": "embedding",
  "output_format": "json"
}
```

`output_format` 当前支持：

- `json`
- `json_rows`
- `arrow`
- `arrow_ipc`

返回 JSON 示例：

```json
{
  "success": true,
  "message": "search completed",
  "format": "json",
  "rows": 3,
  "byte_length": 256
}
```

真正的结果数据在：

- `output_data->data`
- `output_data->len`
- `output_data->cap` 仅供释放函数内部使用，调用方不要修改

调用方读取完后，必须执行：

```c
vldb_lancedb_bytes_free(output_data_value);
```

### 7.3.1 非 JSON 主检索接口

```c
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
```

说明：

- 这是当前推荐给 Go / 原生宿主的主检索接口
- 查询向量通过 `float* + len` 直接传入，不再走 JSON
- 输出结果仍然写入 `VldbLancedbByteBuffer`
- `out_result` 返回：
  - `format`
  - `rows`
  - `byte_length`

成功时：

- 返回 `VLDB_LANCEDB_STATUS_SUCCESS`
- `output_data` 中保存原始结果 bytes
- `out_result` 中保存结果元信息

失败时：

- 返回 `VLDB_LANCEDB_STATUS_FAILURE`
- 通过 `vldb_lancedb_last_error_message()` 读取错误信息

### 7.4 删除

```c
char* vldb_lancedb_engine_delete_json(
    VldbLancedbEngineHandle* handle,
    const char* input_json
);
```

输入 JSON：

```json
{
  "table_name": "memory",
  "condition": "id = '1'"
}
```

返回 JSON 示例：

```json
{
  "success": true,
  "message": "delete completed",
  "version": 4,
  "deleted_rows": 1
}
```

### 7.5 删表

```c
char* vldb_lancedb_engine_drop_table_json(
    VldbLancedbEngineHandle* handle,
    const char* input_json
);
```

输入 JSON：

```json
{
  "table_name": "memory"
}
```

返回 JSON 示例：

```json
{
  "success": true,
  "message": "table dropped"
}
```

### 7.6 释放 Engine

```c
void vldb_lancedb_engine_destroy(VldbLancedbEngineHandle* handle);
```

每个成功打开的引擎句柄最终都必须显式释放。

---

## 8. 错误处理与资源释放

### 8.1 最近一次错误

```c
const char* vldb_lancedb_last_error_message(void);
void vldb_lancedb_clear_last_error(void);
```

规则：

- 当某个 FFI 接口返回 `NULL` 时，应立即读取最近一次错误
- 该错误文本是线程局部的
- 如果你已经消费过错误并完成记录，可调用 `clear`

### 8.2 字符串释放

下列接口返回的 `char*` 都必须通过：

```c
void vldb_lancedb_string_free(char* value);
```

释放：

- `vldb_lancedb_runtime_database_path_for_name`
- 所有 `*_json` 返回的结果字符串

### 8.3 字节释放

检索返回的 `VldbLancedbByteBuffer` 必须通过：

```c
void vldb_lancedb_bytes_free(VldbLancedbByteBuffer buffer);
```

释放。`cap` 字段必须保留为库返回的原值，不能由调用方自行改写。

### 8.4 空句柄判断

```c
uint8_t vldb_lancedb_engine_is_null(const VldbLancedbEngineHandle* handle);
uint8_t vldb_lancedb_runtime_is_null(const VldbLancedbRuntimeHandle* handle);
```

这两个接口主要用于调用方做防御式判断或语言绑定层包装。

---

## 9. 推荐生命周期

推荐调用顺序如下：

1. 取默认选项模板
2. 覆盖 `default_db_path` / `db_root` / 上限参数
3. 创建 `Runtime`
4. 打开默认库或命名库的 `Engine`
5. 执行业务操作
6. 释放所有返回字符串和结果字节
7. 销毁 `Engine`
8. 销毁 `Runtime`

---

## 10. 最小 FFI 示例（C 风格伪代码）

```c
VldbLancedbRuntimeOptions options = vldb_lancedb_runtime_options_default();
options.default_db_path = "./data/default";
options.db_root = "./data/databases";

VldbLancedbRuntimeHandle* runtime = vldb_lancedb_runtime_create(options);
if (runtime == NULL) {
    printf("runtime create failed: %s\n", vldb_lancedb_last_error_message());
    return;
}

VldbLancedbEngineHandle* engine = vldb_lancedb_runtime_open_default_engine(runtime);
if (engine == NULL) {
    printf("open engine failed: %s\n", vldb_lancedb_last_error_message());
    vldb_lancedb_runtime_destroy(runtime);
    return;
}

char* create_result = vldb_lancedb_engine_create_table_json(engine, create_json);
if (create_result == NULL) {
    printf("create table failed: %s\n", vldb_lancedb_last_error_message());
} else {
    printf("create table: %s\n", create_result);
    vldb_lancedb_string_free(create_result);
}

vldb_lancedb_engine_destroy(engine);
vldb_lancedb_runtime_destroy(runtime);
```

---

## 11. 接入建议

如果你的上层是 Rust：

- 优先直接依赖 `lib`
- 从 `runtime::LanceDbRuntime` 和 `engine::LanceDbEngine` 开始
- 不建议为了“统一风格”而让 Rust 侧再经过 JSON FFI 绕行

如果你的上层不是 Rust，而是：

- Lua 宿主
- MCP 框架
- Manager/Launcher
- 本地桌面程序

建议做一层你自己的轻量包装，而不是直接把所有 FFI 细节暴露给业务代码。推荐包装内容：

- 统一错误转换
- 自动释放字符串/字节
- Runtime / Engine 生命周期托管
- JSON 编解码辅助
- 默认选项模板填充

这样可以避免业务层直接处理：

- `NULL` 指针
- `char*` 释放
- `VldbLancedbByteBuffer` 释放
- 最近一次错误缓冲区

如果你的上层是 Go：

- 优先使用 `vldb_lancedb_engine_vector_upsert_raw`
- 优先使用 `vldb_lancedb_engine_vector_search_f32`
- 将 `*_json` 视为兼容层，而不是主调用路径
- 可直接参考：
  - [../examples/go-ffi/README.md](../examples/go-ffi/README.md)
  - [../examples/go-ffi/lancedbffi/lancedbffi.go](../examples/go-ffi/lancedbffi/lancedbffi.go)

---

## 12. 当前文档与实现的对应关系

本文说明只覆盖**当前已经实现并导出的接口**，不预写未来能力。  
若后续新增：

- 额外 engine 操作
- 新输入格式
- 新输出格式
- 新运行时控制能力

应同步更新：

- `include/vldb_lancedb.h`
- 本文档

以保证动态库调用方始终可以按文档直接对接。
