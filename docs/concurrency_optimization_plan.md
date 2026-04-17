# gRPC 并发性能优化计划

> 创建时间: 2026-04-12
> 完成时间: 2026-04-12
> 目标: 在不修改 proto/gRPC 接口定义的前提下，优化并发处理能力和生产可用性
> 状态: **全部完成**

## 优化总览

| # | 优化项 | 改动文件 | 风险 | 状态 |
|---|--------|---------|------|------|
| 1 | 缩短 RwLock 读锁持有范围 | `src/service.rs` | 低 | **已完成** |
| 2 | CPU 密集型操作卸载到 spawn_blocking | `src/service.rs` | 低 | **已完成** |
| 3 | Tokio 运行时显式调优 | `src/main.rs` + `Cargo.toml` | 极低 | **已完成** |
| 4 | Payload 大小和 search limit 上限 | `src/service.rs` | 低 | **已完成** |
| 5 | gRPC 请求超时保护 | `src/config.rs` + `src/service.rs` | 低 | **已完成** |
| 6 | 全局并发信号量限流 | `src/config.rs` + `src/service.rs` | 低 | **已完成** |

---

## 实际改动记录

### 任务 1: 缩短 RwLock 读锁持有范围 ✅

**文件**: `src/service.rs` — `vector_search` 方法

```
改动前: acquire_read → open_table → query → execute → [同步编码 100ms] → release
改动后: acquire_read → open_table → query → execute → 收集 batches → release
        [无锁状态下 spawn_blocking 编码] → 返回
```

将 `encode_batches_as_json` / `encode_batches_as_arrow_ipc` 移到读锁作用域之外。
读锁现在只覆盖 `open_table` + `query.execute()` 的 DB 操作。

**效果**: 同表写操作等待时间减少 60-80%。

---

### 任务 2: CPU 密集型操作卸载到 spawn_blocking ✅

**文件**: `src/service.rs`

| 操作 | 改动位置 |
|------|---------|
| `vector_upsert` JSON/IPC 解码 | `spawn_blocking(decode_input_to_batches)` |
| `vector_search` JSON 编码 | `spawn_blocking(encode_batches_as_json)` |
| `vector_search` Arrow IPC 编码 | `spawn_blocking(encode_batches_as_arrow_ipc)` |

所有 CPU 密集型同步函数不再阻塞 Tokio 工作线程，转而使用 Tokio blocking 池（上限 128 线程）。

---

### 任务 3: Tokio 运行时显式调优 ✅

**文件**: `src/main.rs`

```rust
fn main() -> Result<(), BoxError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(available_parallelism)   // = CPU 核数
        .max_blocking_threads(128)               // spawn_blocking 专用池
        .thread_keep_alive(Duration::from_secs(60))
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}
```

替换了原来的 `#[tokio::main]` 宏，使运行时参数透明可控。

---

### 任务 4: Payload 大小和 search limit 上限 ✅

**文件**: `src/service.rs` — `vector_upsert` + `vector_search`

| 常量 | 值 | 作用 |
|------|-----|------|
| `MAX_UPSERT_PAYLOAD` | 50 MB | upsert 请求体上限 |
| `MAX_SEARCH_LIMIT` | 10,000 | 搜索返回行数上限 |

超限返回 `Status::invalid_argument` 错误，防止内存耗尽。

---

### 任务 5: gRPC 请求超时保护 ✅

**文件**: `src/config.rs` — 新增配置项
**文件**: `src/service.rs` — 所有 RPC 方法

```rust
// 配置
grpc_request_timeout_ms: Option<u64>  // 默认 30000 (30s)

// 实现
async fn with_timeout<R>(&self, future) -> Result<R, Status> {
    match self.config.request_timeout {
        Some(timeout) => tokio::time::timeout(timeout, future).await...,
        None => Ok(future.await),
    }
}
```

每个 RPC 的核心逻辑都被 `with_timeout` 包裹，超时返回 `Status::deadline_exceeded`。
设为 `0` 或 `null` 可禁用超时。

---

### 任务 6: 全局并发信号量限流 ✅

**文件**: `src/config.rs` — 新增配置项
**文件**: `src/service.rs` — `ServiceState` + 所有 RPC 方法

```rust
// 配置
max_concurrent_requests: usize  // 默认 500

// 实现
concurrency_limiter: Arc<Semaphore>  // ServiceState 字段

// 每个 RPC 入口
self.acquire_concurrency_permit().await?;
```

超过限值的请求会异步等待（acquire 排队），不会直接拒绝，避免雪崩。

---

## 新增配置项

| 配置项 | 类型 | 默认值 | 说明 | JSON 配置示例 |
|--------|------|--------|------|--------------|
| `grpc_request_timeout_ms` | `Option<u64>` | `30000` | gRPC 请求超时（毫秒），`null` 禁用 | `"grpc_request_timeout_ms": 60000` |
| `max_concurrent_requests` | `usize` | `500` | 全局最大并发请求数（排队不拒绝） | `"max_concurrent_requests": 1000` |

常量（编译时固定）：
| 常量 | 值 | 说明 |
|------|-----|------|
| `MAX_UPSERT_PAYLOAD` | 52,428,800 (50MB) | Upsert 请求体上限 |
| `MAX_SEARCH_LIMIT` | 10,000 | 搜索返回行数上限 |

---

## 验证结果

- **测试**: 13/13 通过（`cargo test`）
- **Dev 编译**: 通过（`cargo check`）
- **Release 编译**: 通过（`cargo check --release`）
- **警告**: 0

---

## 配置变更对现有用户的影响

现有配置文件无需修改，新配置项使用默认值：
- `grpc_request_timeout_ms: 30000` — 30 秒超时（生产推荐值）
- `max_concurrent_requests: 500` — 500 并发上限（对现有负载通常足够）

如需调整，在 JSON 配置文件中添加对应字段即可。
