# vldb-lancedb

一个基于 Rust + gRPC 的 LanceDB 向量数据网关。通过 gRPC 协议对外提供建表、写入、向量检索、删除和删表五类能力，使任意语言可将 LanceDB 作为独立向量服务使用。

当前仓库同时支持：
- `bin` 入口：直接启动 gRPC 服务
- `lib` 入口：提供纯程序化的 LanceDB 核心能力、运行时入口与多库管理器，便于本地 Rust/Lua 上层二次封装
- `ffi` 入口：提供稳定 C ABI 与头文件，便于非 Rust 宿主加载动态库

当前推荐的调用分层是：
- **Rust / MCP**
  - 优先直接使用 `lib` 暴露的 typed Rust API
  - 不建议再绕行 JSON FFI
- **Go / 其他原生宿主**
  - 优先使用 FFI 的非 JSON 主接口
  - 高频向量写入与检索不建议再走 JSON
- **JSON FFI**
  - 保留为兼容层，主要服务脚本语言、调试与其他潜在项目

## 快速开始

```bash
cargo build --release
cargo run --release -- --config ./vldb-lancedb.json
```

默认监听: `127.0.0.1:19301`

## 详细文档

| 语言 | 链接 |
|------|------|
| 中文完整指南 | [docs/README.zh-CN.md](./docs/README.zh-CN.md) |
| 库模式与 FFI 接入说明 | [docs/LIBRARY_USAGE.zh-CN.md](./docs/LIBRARY_USAGE.zh-CN.md) |
| English Guide | [docs/README.en.md](./docs/README.en.md) |
| gRPC 协议定义 | [proto/v1/lancedb.proto](./proto/v1/lancedb.proto) |
| Go 示例客户端 | [examples/go-client/](./examples/go-client/) |
| Go FFI 示例 | [examples/go-ffi/README.md](./examples/go-ffi/README.md) |
| 并发优化计划 | [docs/concurrency_optimization_plan.md](./docs/concurrency_optimization_plan.md) |

## 功能概览

| RPC | 用途 | 输入格式 | 输出格式 |
|-----|------|---------|---------|
| `CreateTable` | 创建向量表，定义标量列和向量列 | — | — |
| `VectorUpsert` | 追加或合并写入向量数据 | JSON Rows / Arrow IPC | — |
| `VectorSearch` | 最近邻向量检索 | — | JSON Rows / Arrow IPC |
| `Delete` | 按条件删除记录 | 谓词字符串 | — |
| `DropTable` | 删除整张表 | — | — |

## 支持的标量类型

`STRING` / `INT32` / `INT64` / `UINT32` / `UINT64` / `FLOAT32` / `FLOAT64` / `BOOL` / `VECTOR_FLOAT32`

## 当前状态

- **已完成**: 5 个核心 RPC、per-table 并发控制、请求超时保护、并发限流、payload 防护、Docker 支持
- **已知问题**: 无 TLS/认证、单表写入串行化、JSON 编解码存在类型转换精度损失
- **详见**: [docs/README.zh-CN.md#当前状态与已知问题](./docs/README.zh-CN.md#当前状态与已知问题)

## 优化概览

已完成 6 项并发性能优化（不修改 proto 接口）:

| 优化项 | 效果 |
|--------|------|
| 缩短读锁范围 | 写等待减少 60-80% |
| spawn_blocking 卸载 CPU 操作 | 不饿死 Tokio 线程 |
| 显式 Tokio 运行时 | 参数透明可控 |
| Payload & limit 上限 | 防止 OOM |
| 请求超时 | p99 延迟可控 |
| 并发信号量 | 防止雪崩 |

并发能力预估（8 核机器）:
- 纯读（小结果集）: **800-3000 QPS**
- 纯读（大结果集）: **300-800 QPS**
- 混合读写 (9:1): **600-2500 QPS**

详见: [docs/concurrency_optimization_plan.md](./docs/concurrency_optimization_plan.md)

## 对接注意事项

- 无 TLS/认证，建议通过反向代理或内网隔离保障安全
- `SearchResponse.data` 上限受 `MAX_SEARCH_LIMIT=10000` 行限制
- `UpsertRequest.data` 上限 50MB
- 所有请求超时默认 30 秒（可配置）
- 默认日志写入 `<db_path>/logs/`
- 多实例场景下 `s3://` 不安全，请改用 `s3+ddb://`

## 配置文件

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

说明：
- `db_path`：默认数据库路径，也是现有 gRPC 服务启动时直接绑定的数据库
- 多库能力仅通过 `lib` 的程序化 API 暴露，不改变现有 gRPC 服务配置协议

## Library 使用方向

当前 `lib` 建议调用顺序如下：

- 使用 `runtime::LanceDbRuntime` 创建嵌入式运行时
- 通过运行时打开默认库或命名库的 `engine`
- 使用 `engine` 执行建表、写入、检索、删除、删表等核心操作

这样调用方无需自己手动拼装 `DatabaseManager + LanceDbEngineOptions + LanceDbEngine`。

如果需要完整的库模式与 FFI 接入说明，请直接查看：

- [docs/LIBRARY_USAGE.zh-CN.md](./docs/LIBRARY_USAGE.zh-CN.md)

## FFI 使用方向

当前仓库已经提供：

- 头文件：`include/vldb_lancedb.h`
- 动态库导出：通过 `cdylib` 产出 `vldb_lancedb.dll/.so/.dylib`

当前 FFI 已暴露的能力面包括：

- 创建运行时
- 打开默认库或命名库的引擎句柄
- 解析数据库路径
- 建表
- 写入
- 检索
- 删除
- 删表
- 获取最近一次错误
- 释放运行时、引擎与字符串资源
- 释放原始结果字节缓冲区

其中：

- **主路径（推荐给 Go / 原生宿主）**
  - `vldb_lancedb_engine_vector_upsert_raw`
  - `vldb_lancedb_engine_vector_search_f32`
  - 使用扁平参数与固定结构体，不依赖业务 JSON
- **兼容路径**
  - `*_json`
  - 继续保留，方便 Lua / Python / 调试快速接入

兼容层当前采用：

- `JSON`：承载建表、检索、删除、删表等元信息输入
- `bytes`：承载写入 payload 与检索原始结果

这样可以在保持 ABI 简洁的前提下，让 Lua/MCP/manager 等非 Rust 调用方直接完成核心表操作。

### FFI runtime 修复说明

2026-04 的这一轮修复中，FFI 层已经把异步执行模型调整为：

- 固定后台 worker 线程
- 独占 `current_thread` Tokio runtime
- 所有 FFI async 调用通过 worker 串行调度

这次调整的目的，是修复 Go / purego / 多线程宿主下可能出现的：

- `EnterGuard values dropped out of order`

兼容性结论：

- **不影响现有导出符号**
- **不影响头文件**
- **不要求调用方修改现有函数签名或参数结构**
- **不影响 gRPC 服务模式**
- **不影响 Rust 直接嵌入 `lib` API 的调用方式**

需要注意的一点是：

- FFI 调用入口现在会在 worker 线程上串行调度
- 因此极端高并发 FFI 宿主的跨线程并行吞吐，理论上可能低于旧实现
- 但换来的是更稳定的运行时生命周期与跨语言集成行为

如果需要完整背景、调用模型和集成影响说明，请继续参考：

- [docs/LIBRARY_USAGE.zh-CN.md](./docs/LIBRARY_USAGE.zh-CN.md)

更完整的内容包括：

- 动态库产物路径
- 头文件使用方式
- Runtime / Engine 生命周期
- 字符串与字节缓冲区释放规则
- JSON 输入结构与结果示例

详见：

- [docs/LIBRARY_USAGE.zh-CN.md](./docs/LIBRARY_USAGE.zh-CN.md)

## 环境要求

- Rust `1.94.0`
- `protoc`（编译时必须）
- Go `1.24+`（仅运行 Go 示例时需要）

## Docker

```bash
docker build -t vulcan/vldb-lancedb:local .
docker run -d --name vldb-lancedb -p 19301:19301 -v vldb-lancedb-data:/app/data vulcan/vldb-lancedb:local
```
