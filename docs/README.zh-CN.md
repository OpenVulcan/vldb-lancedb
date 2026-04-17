# vldb-lancedb 中文完整指南

## 项目简介

`vldb-lancedb` 是一个基于 Rust 和 gRPC 的 LanceDB 向量数据网关，将 LanceDB 封装为独立服务，使其他语言可通过 gRPC 协议进行向量数据的建表、写入、检索和删除。

### 架构概览

```
客户端 (任意语言)
    │ gRPC (HTTP/2)
    ▼
vldb-lancedb 服务 (Rust + Tokio)
    ├── Tokio 多线程运行时 (工作线程 = CPU 核数)
    ├── Blocking 池 (128 线程，用于 CPU 密集型操作)
    ├── 并发信号量 (默认 500 permit)
    ├── 请求超时 (默认 30s)
    └── per-table RwLock 协调器
            │
            ▼
        LanceDB Connection (Arc 共享)
            │
            ▼
        LanceDB 本地文件系统 / S3
```

## 目录

- [环境要求](#环境要求)
- [构建与启动](#构建与启动)
- [Docker 部署](#docker-部署)
- [配置说明](#配置说明)
- [库模式与 FFI 接入](#库模式与-ffi-接入)
- [gRPC 服务对接](#grpc-服务对接)
  - [1. CreateTable](#1-createtable)
  - [2. VectorUpsert](#2-vectorupsert)
  - [3. VectorSearch](#3-vectorsearch)
  - [4. Delete](#4-delete)
  - [5. DropTable](#5-droptable)
- [数据类型映射](#数据类型映射)
- [Go 示例客户端](#go-示例客户端)
- [当前状态与已知问题](#当前状态与已知问题)
- [并发能力与优化](#并发能力与优化)
- [对接注意事项](#对接注意事项)

---

## 环境要求

| 依赖 | 版本 | 说明 |
|------|------|------|
| Rust | `1.94.0` | 编译和运行 |
| `protoc` | 任意现代版本 | 编译时必须，用于生成 gRPC 代码 |
| Go | `1.24+` | 仅运行 Go 示例客户端时需要 |

> **提示**: `lance-*` 依赖链在编译阶段会使用 `protoc`。如果系统未安装 `protoc`，构建会失败。可设置 `PROTOC` 环境变量指向自定义路径。

---

## 构建与启动

```bash
# 开发构建
cargo build

# 生产构建（LTO 优化）
cargo build --release

# 启动服务
cargo run --release -- --config ./vldb-lancedb.json
```

配置发现顺序:
1. `--config <path>` 或 `-config <path>`
2. 可执行文件目录下的 `vldb-lancedb.json`
3. 可执行文件目录下的 `lancedb.json`
4. 运行目录下的 `vldb-lancedb.json`
5. 运行目录下的 `lancedb.json`
6. 内置默认值

---

## Docker 部署

构建镜像:

```bash
docker build -t vulcan/vldb-lancedb:local .
```

启动容器:

```bash
docker run -d \
  --name vldb-lancedb \
  -p 19301:19301 \
  -v vldb-lancedb-data:/app/data \
  -v ./vldb-lancedb/docker/vldb-lancedb.json:/app/config/vldb-lancedb.json:ro \
  vulcan/vldb-lancedb:local
```

| 项目 | 说明 |
|------|------|
| 监听地址 | `0.0.0.0:19301`（容器内） |
| 数据路径 | `/app/data`（容器内） |
| 配置文件 | `/app/config/vldb-lancedb.json` |
| 数据持久化 | 推荐使用 Docker 命名卷 |

---

## 配置说明

完整 JSON 配置示例:

```json
{
  "host": "127.0.0.1",
  "port": 19301,
  "db_path": "./data",
  "read_consistency_interval_ms": 0,
  "grpc_request_timeout_ms": 30000,
  "max_concurrent_requests": 500,
  "logging": {
    "enabled": true,
    "file_enabled": true,
    "stderr_enabled": true,
    "request_log_enabled": true,
    "slow_request_log_enabled": true,
    "slow_request_threshold_ms": 1000,
    "include_request_details_in_slow_log": true,
    "request_preview_chars": 160,
    "log_dir": "",
    "log_file_name": "vldb-lancedb.log"
  }
}
```

### 核心字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `host` | `string` | `"127.0.0.1"` | gRPC 监听地址 |
| `port` | `u16` | `19301` | gRPC 监听端口 |
| `db_path` | `string` | `"./data"` | LanceDB 数据目录或远程 URI |
| `read_consistency_interval_ms` | `Option<u64>` | `0` | 跨进程刷新间隔；`0`=强一致，非零=最终一致，`null`=禁用跨进程刷新 |
| `grpc_request_timeout_ms` | `Option<u64>` | `30000` | 请求超时（毫秒）；`null` 表示禁用 |
| `max_concurrent_requests` | `usize` | `500` | 全局最大并发请求数（排队等待，不直接拒绝） |

### 日志字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `logging.enabled` | `bool` | `true` | 日志总开关 |
| `logging.file_enabled` | `bool` | `true` | 是否写入文件 |
| `logging.stderr_enabled` | `bool` | `true` | 是否输出到 stderr |
| `logging.request_log_enabled` | `bool` | `true` | 是否记录每次请求 |
| `logging.slow_request_log_enabled` | `bool` | `true` | 是否记录慢请求 |
| `logging.slow_request_threshold_ms` | `u64` | `1000` | 慢请求阈值 |
| `logging.include_request_details_in_slow_log` | `bool` | `true` | 慢请求日志是否包含请求摘要 |
| `logging.request_preview_chars` | `usize` | `160` | 请求摘要最大预览长度 |
| `logging.log_dir` | `string` | `""` | 自定义日志目录；为空时使用 `<db_path>/logs/` |
| `logging.log_file_name` | `string` | `"vldb-lancedb.log"` | 日志文件名（自动加日期） |

### 路径规则

- 相对路径以配置文件所在目录为基准
- 含 `://` 的值视为 URI，原样使用
- 本地目录不存在时自动创建
- 日志按天分离为 `vldb-lancedb_YYYY-MM-DD.log`

### 存储后端注意事项

- `db_path` 为本地路径时，服务自动创建目录
- 使用 `s3://` 时，启动时会告警（多写入者不安全）
- 多实例/多写入者场景请改用 `s3+ddb://`

---

## 库模式与 FFI 接入

除了 gRPC 服务模式外，`vldb-lancedb` 当前还支持：

- `lib`
  - 供 Rust 工程直接嵌入
- `ffi`
  - 供 Lua / MCP / manager / 其他本地宿主加载动态库

这两种模式的共同特点是：

- 不依赖配置文件发现
- 不依赖 gRPC 请求结构
- 由调用方自己决定运行时参数、生命周期和上层业务流程

如果您需要查看以下内容：

- 动态库文件位置
- 头文件说明
- Runtime / Engine 句柄的创建与销毁
- 字符串与字节缓冲区释放规则
- 建表 / 写入 / 检索 / 删除 / 删表的 JSON 输入结构
- 一段完整的典型调用顺序

请直接参考：

- [库模式与 FFI 接入说明](./LIBRARY_USAGE.zh-CN.md)

---

## gRPC 服务对接

gRPC 服务名: `vldb.lancedb.v1.LanceDbService`

所有 RPC 均为 **unary**（非流式）调用。

### 1. CreateTable

创建新表，定义标量列和向量列。

**请求**: `CreateTableRequest`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | `string` | 是 | 表名 |
| `columns` | `repeated ColumnDef` | 是 | 列定义列表 |
| `overwrite_if_exists` | `bool` | 否 | 表存在时是否覆盖重建 |

**ColumnDef**:

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `string` | 是 | 列名 |
| `column_type` | `ColumnType` | 是 | 列类型枚举 |
| `vector_dim` | `uint32` | 向量列必填 | 向量维度（必须 > 0） |
| `nullable` | `bool` | 否 | 是否可为空 |

**ColumnType 枚举**:

| 枚举值 | 说明 |
|--------|------|
| `COLUMN_TYPE_UNSPECIFIED` | 未指定（非法） |
| `COLUMN_TYPE_STRING` | 字符串 |
| `COLUMN_TYPE_INT32` | 32 位有符号整数 |
| `COLUMN_TYPE_INT64` | 64 位有符号整数 |
| `COLUMN_TYPE_UINT32` | 32 位无符号整数 |
| `COLUMN_TYPE_UINT64` | 64 位无符号整数 |
| `COLUMN_TYPE_FLOAT32` | 32 位浮点数 |
| `COLUMN_TYPE_FLOAT64` | 64 位浮点数 |
| `COLUMN_TYPE_BOOL` | 布尔值 |
| `COLUMN_TYPE_VECTOR_FLOAT32` | Float32 固定长度向量 |

**响应**: `CreateTableResponse`

| 字段 | 类型 | 说明 |
|------|------|------|
| `success` | `bool` | 是否成功 |
| `message` | `string` | 描述信息 |

**错误场景**:
- `table_name` 为空
- `columns` 为空
- 向量列 `vector_dim` 为 0
- 表已存在且 `overwrite_if_exists=false`

---

### 2. VectorUpsert

向已有表追加或合并写入向量数据。

**请求**: `UpsertRequest`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | `string` | 是 | 表名 |
| `input_format` | `InputFormat` | 否 | 输入数据格式 |
| `data` | `bytes` | 是 | 数据负载 |
| `key_columns` | `repeated string` | 否 | 合并键列名 |

**InputFormat 枚举**:

| 枚举值 | 说明 |
|--------|------|
| `INPUT_FORMAT_UNSPECIFIED` | 默认，视为 `JSON_ROWS` |
| `INPUT_FORMAT_JSON_ROWS` | JSON 数组，每行一个对象 |
| `INPUT_FORMAT_ARROW_IPC` | Arrow IPC 格式二进制数据 |

**行为**:

| `key_columns` | 行为 |
|---------------|------|
| 空 | 追加模式（Append） |
| 非空 | 合并模式（Merge Upsert）— 匹配则更新，不匹配则插入 |

**JSON 写入要求**:
- `data` 必须是 JSON 数组，如 `[{"id":1,"vec":[0.1,...]},{"id":2,...}]`
- 每行必须为 JSON 对象
- 向量字段必须是浮点数数组，如 `[0.1, 0.2, ..., 0.768]`
- 非空字段不能缺失
- JSON 字段类型必须与表结构匹配

**响应**: `UpsertResponse`

| 字段 | 类型 | 说明 |
|------|------|------|
| `success` | `bool` | 是否成功 |
| `message` | `string` | 描述信息 |
| `version` | `uint64` | 操作后表版本 |
| `input_rows` | `uint64` | 输入行数 |
| `inserted_rows` | `uint64` | 新增行数 |
| `updated_rows` | `uint64` | 更新行数 |
| `deleted_rows` | `uint64` | 删除行数 |

**限制**:
- `data` 大小上限 **50MB**（超限返回 `INVALID_ARGUMENT`）

---

### 3. VectorSearch

执行最近邻向量检索。

**请求**: `SearchRequest`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | `string` | 是 | 表名 |
| `vector` | `repeated float` | 是 | 查询向量 |
| `limit` | `uint32` | 否 | 返回行数上限；`0` 时默认 10 |
| `filter` | `string` | 否 | 附加过滤条件，如 `active = true` |
| `vector_column` | `string` | 否 | 指定用于检索的向量列名 |
| `output_format` | `OutputFormat` | 否 | 输出数据格式 |

**OutputFormat 枚举**:

| 枚举值 | 说明 |
|--------|------|
| `OUTPUT_FORMAT_UNSPECIFIED` | 默认，使用 `ARROW_IPC` |
| `OUTPUT_FORMAT_ARROW_IPC` | Arrow IPC 二进制 |
| `OUTPUT_FORMAT_JSON_ROWS` | JSON 数组 |

**响应**: `SearchResponse`

| 字段 | 类型 | 说明 |
|------|------|------|
| `success` | `bool` | 是否成功 |
| `message` | `string` | 描述信息 |
| `format` | `string` | 实际输出格式（`"json"` 或 `"arrow_ipc"`） |
| `rows` | `uint64` | 返回行数 |
| `data` | `bytes` | 结果数据 |

**限制**:
- `vector` 不能为空
- `limit` 上限 **10,000**（客户端传更大也会被截断）
- JSON 输出可能包含 `_distance` 字段

**错误场景**:
- `table_name` 为空
- `vector` 为空

---

### 4. Delete

按条件删除表中记录。

**请求**: `DeleteRequest`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | `string` | 是 | 表名 |
| `condition` | `string` | 是 | 过滤条件 |

**条件示例**:
- `session_id = 'abc'`
- `id >= 100`
- `created_at < '2025-01-01'`

> `condition` 直接传递给 LanceDB 作为过滤表达式，调用方需保证条件字符串语法正确。

**响应**: `DeleteResponse`

| 字段 | 类型 | 说明 |
|------|------|------|
| `success` | `bool` | 是否成功 |
| `message` | `string` | 描述信息 |
| `version` | `uint64` | 操作后表版本 |
| `deleted_rows` | `uint64` | 删除行数 |

---

### 5. DropTable

删除整张 LanceDB 表。

**请求**: `DropTableRequest`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | `string` | 是 | 表名 |

**响应**: `DropTableResponse`

| 字段 | 类型 | 说明 |
|------|------|------|
| `success` | `bool` | 是否成功 |
| `message` | `string` | 描述信息 |

---

## 数据类型映射

### JSON ↔ Arrow 映射

| Arrow 类型 | JSON 类型 | 说明 |
|-----------|-----------|------|
| `Utf8` | 字符串 | `"hello"` |
| `Int32` | 整数 | `42` |
| `Int64` | 整数 | `42` |
| `UInt32` | 整数 | `42` |
| `UInt64` | 整数 | `42` |
| `Float32` | 数字 | `3.14` |
| `Float64` | 数字 | `3.14` |
| `Boolean` | 布尔值 | `true` / `false` |
| `FixedSizeList(Float32, dim)` | 浮点数数组 | `[0.1, 0.2, ..., 0.768]` |

### 写入时 JSON → Arrow 解码规则

- 向量字段 (`FixedSizeList`): 必须为 JSON 数组，元素数量必须等于声明维度
- 可空字段缺失时自动填充 `null`
- 非空字段缺失时报错
- 类型不匹配时报错（如字符串传给了整数字段）

### 读取时 Arrow → JSON 编码规则

- `FixedSizeList(Float32)`: 编码为浮点数数组
- 浮点数以 `f64` 精度输出（`Float32` 值转为 `f64`）
- `null` 值输出为 JSON `null`
- 向量检索结果可能包含 `_distance` 字段

---

## Go 示例客户端

生成 Go stubs:

```bash
go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.11
go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.1

protoc \
  -I . \
  --go_out=./examples/go-client/gen \
  --go_opt=paths=source_relative \
  --go-grpc_out=./examples/go-client/gen \
  --go-grpc_opt=paths=source_relative \
  ./proto/v1/lancedb.proto
```

运行示例:

```bash
cd ./examples/go-client
go mod tidy
go run .
```

示例执行流程: `CreateTable` → `VectorUpsert` → `VectorSearch` → `Delete` → `DropTable`

---

## 当前状态与已知问题

### 已完成功能

- [x] 5 个核心 RPC（CreateTable / VectorUpsert / VectorSearch / Delete / DropTable）
- [x] JSON Rows 和 Arrow IPC 双向编解码
- [x] per-table 读写并发协调（RwLock）
- [x] 请求超时保护（可配置）
- [x] 全局并发信号量限流（可配置）
- [x] Payload 大小保护（50MB 上限）
- [x] Search limit 上限（10,000 行）
- [x] CPU 密集型操作卸载到 blocking 池
- [x] 请求日志、慢请求日志、日志自动轮转
- [x] Docker 支持
- [x] S3 / S3+DynamoDB 远程存储支持

### 已知问题与局限

| 问题 | 影响 | 严重性 |
|------|------|--------|
| **无 TLS/认证/鉴权** | 网络层暴露风险 | 高 |
| **单表写入完全串行化** | 同表 upsert 无法并行，单表写入 QPS 上限 ~60 | 中 |
| **JSON 编解码精度损失** | Float32 在 JSON 中以 f64 输出，Float64 输入可能被截断为 Float32 | 低 |
| **无请求重试机制** | 网络抖动或 LanceDB 临时错误直接返回客户端 | 中 |
| **无熔断/降级** | 下游 LanceDB 故障时请求持续排队直到超时 | 中 |
| **日志 Mutex 全局竞争** | >2000 QPS 时日志序列化增加 p99 延迟 2-5ms | 低 |
| **单客户端可占满全部 permit** | 无 per-connection 限流 | 中 |
| **普通 `s3://` 不支持多写入者** | 多实例并发写可能导致元数据损坏 | 高（仅限 S3 场景） |

### 已完成优化项

| # | 优化项 | 效果 | 详见 |
|---|--------|------|------|
| 1 | 缩短 RwLock 读锁持有范围 | 写等待减少 60-80% | [并发优化计划](./concurrency_optimization_plan.md#任务-1-缩短-rwlock-读锁持有范围-) |
| 2 | CPU 密集型操作卸载到 spawn_blocking | 不饿死 Tokio 工作线程 | [并发优化计划](./concurrency_optimization_plan.md#任务-2-cpu-密集型操作卸载到-spawn_blocking-) |
| 3 | Tokio 运行时显式调优 | 参数透明可控 | [并发优化计划](./concurrency_optimization_plan.md#任务-3-tokio-运行时显式调优-) |
| 4 | Payload 大小和 search limit 上限 | 防止 OOM | [并发优化计划](./concurrency_optimization_plan.md#任务-4-payload-大小和-search-limit-上限-) |
| 5 | gRPC 请求超时保护 | p99 延迟可控 | [并发优化计划](./concurrency_optimization_plan.md#任务-5-grpc-请求超时保护-) |
| 6 | 全局并发信号量限流 | 防止雪崩 | [并发优化计划](./concurrency_optimization_plan.md#任务-6-全局并发信号量限流-) |

### 并发能力预估（8 核机器，优化后）

| 场景 | 预估 QPS | 说明 |
|------|---------|------|
| 纯读（小结果集 limit≤10） | 800-3000 | 瓶颈在 Tokio 线程数 + 磁盘 I/O |
| 纯读（大结果集 limit≤1000） | 300-800 | 编码在 blocking 池，不阻塞工作线程 |
| 纯读（超大结果集 limit≤10000） | 100-300 | 受 MAX_SEARCH_LIMIT 保护 |
| 单表写入（小批量 100 行） | 25-60 | 写锁串行化，受 LanceDB 写入速度限制 |
| 混合读写 (9:1) | 600-2500 | 读为主场景，写短暂阻塞 |
| 混合读写 (5:5) | 300-1000 | 读写竞争加剧 |
| 混合读写 (1:9) | 30-120 | 写为主场景，同表串行化 |

> 详细分析见 [docs/concurrency_optimization_plan.md](./concurrency_optimization_plan.md)

---

## 对接注意事项

### 安全

- 服务**不包含** TLS、认证或鉴权层
- 建议通过反向代理（Nginx/Envoy）或内网隔离保障安全
- 监听地址默认 `127.0.0.1`，如需外部访问请显式配置 `0.0.0.0`

### 性能

- 同表写入完全串行化，不同表之间完全并行
- 推荐将高频写入分散到不同表以提升吞吐
- 大结果集搜索的编码操作会占用 blocking 池线程
- 请求超时默认 30 秒，慢查询会自动终止

### 协议

- 所有 RPC 为 unary（非流式），不支持 server streaming 或 bidi streaming
- `UpsertRequest.data` 上限 **50MB**，超出返回 `INVALID_ARGUMENT`
- `SearchRequest.limit` 上限 **10,000**，超出会被静默截断
- `Delete.condition` 原样传给 LanceDB，调用方需保证语法正确
- 表名会自动 `trim()`，首尾空格会被去除

### 运维

- 默认开启请求日志和慢请求日志
- 日志默认写入 `<db_path>/logs/`，按天分离
- 编译环境需要 `protoc`，否则构建失败
- Rust 版本要求 `1.94.0`，旧版本无法编译
- 多实例/多写入者场景请使用 `s3+ddb://` 存储后端

### 调用方示例（伪代码）

```protobuf
// 1. 创建包含向量列的表
CreateTableRequest {
  table_name: "demo"
  columns: [
    { name: "id", column_type: COLUMN_TYPE_INT64, nullable: false },
    { name: "embedding", column_type: COLUMN_TYPE_VECTOR_FLOAT32, vector_dim: 768 }
  ]
}

// 2. JSON 格式写入
UpsertRequest {
  table_name: "demo"
  input_format: INPUT_FORMAT_JSON_ROWS
  data: '[{"id":1,"embedding":[0.1,0.2,...,0.768]},{"id":2,"embedding":[...]}]'
}

// 3. 向量检索
SearchRequest {
  table_name: "demo"
  vector: [0.1, 0.2, ..., 0.768]
  limit: 10
  output_format: OUTPUT_FORMAT_JSON_ROWS
}

// 4. 条件删除
DeleteRequest {
  table_name: "demo"
  condition: "id = 1"
}

// 5. 删表
DropTableRequest {
  table_name: "demo"
}
```
