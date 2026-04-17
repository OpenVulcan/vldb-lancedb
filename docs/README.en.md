# vldb-lancedb English Guide

## Overview

`vldb-lancedb` is a standalone Rust gRPC gateway for LanceDB. It wraps LanceDB as an independent service so that clients in any language can perform table management, vector ingestion, similarity search, and deletion over gRPC.

### Architecture

```
Client (any language)
    │ gRPC (HTTP/2)
    ▼
vldb-lancedb Service (Rust + Tokio)
    ├── Tokio multi-threaded runtime (workers = CPU cores)
    ├── Blocking thread pool (128 threads, for CPU-bound ops)
    ├── Concurrency semaphore (500 permits by default)
    ├── Request timeout (30s by default)
    └── per-table RwLock coordinator
            │
            ▼
        LanceDB Connection (Arc-shared)
            │
            ▼
        LanceDB local filesystem / S3
```

## Table of Contents

- [Requirements](#requirements)
- [Build & Run](#build--run)
- [Docker Deployment](#docker-deployment)
- [Configuration](#configuration)
- [gRPC Service Reference](#grpc-service-reference)
  - [1. CreateTable](#1-createtable)
  - [2. VectorUpsert](#2-vectorupsert)
  - [3. VectorSearch](#3-vectorsearch)
  - [4. Delete](#4-delete)
  - [5. DropTable](#5-droptable)
- [Data Type Mapping](#data-type-mapping)
- [Go Example Client](#go-example-client)
- [Current State & Known Issues](#current-state--known-issues)
- [Concurrency & Optimizations](#concurrency--optimizations)
- [Integration Notes](#integration-notes)

---

## Requirements

| Dependency | Version | Notes |
|-----------|---------|-------|
| Rust | `1.94.0` | Compile and runtime |
| `protoc` | any modern version | Required at compile time for gRPC codegen |
| Go | `1.24+` | Only needed for the Go example client |

> **Note**: the `lance-*` dependency chain invokes `protoc` during builds. If `protoc` is not on `PATH`, set the `PROTOC` environment variable.

---

## Build & Run

```bash
# Development build
cargo build

# Production build (LTO optimized)
cargo build --release

# Start the service
cargo run --release -- --config ./vldb-lancedb.json
```

Config discovery order:
1. `--config <path>` or `-config <path>`
2. `vldb-lancedb.json` in the executable directory
3. `lancedb.json` in the executable directory
4. `vldb-lancedb.json` in the current working directory
5. `lancedb.json` in the current working directory
6. built-in defaults

---

## Docker Deployment

Build the image:

```bash
docker build -t vulcan/vldb-lancedb:local .
```

Run the container:

```bash
docker run -d \
  --name vldb-lancedb \
  -p 19301:19301 \
  -v vldb-lancedb-data:/app/data \
  -v ./vldb-lancedb/docker/vldb-lancedb.json:/app/config/vldb-lancedb.json:ro \
  vulcan/vldb-lancedb:local
```

| Item | Details |
|------|---------|
| Listen address | `0.0.0.0:19301` (inside container) |
| Data path | `/app/data` (inside container) |
| Config path | `/app/config/vldb-lancedb.json` |
| Persistence | Docker named volume recommended |

---

## Configuration

Full JSON configuration example:

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

### Core Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | `string` | `"127.0.0.1"` | gRPC bind address |
| `port` | `u16` | `19301` | gRPC bind port |
| `db_path` | `string` | `"./data"` | LanceDB data directory or remote URI |
| `read_consistency_interval_ms` | `Option<u64>` | `0` | Cross-process refresh interval; `0`=strong consistency, non-zero=eventual, `null`=disabled |
| `grpc_request_timeout_ms` | `Option<u64>` | `30000` | Request timeout in milliseconds; `null` to disable |
| `max_concurrent_requests` | `usize` | `500` | Global max concurrent requests (queues, does not reject) |

### Logging Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `logging.enabled` | `bool` | `true` | Master logging switch |
| `logging.file_enabled` | `bool` | `true` | Write logs to file |
| `logging.stderr_enabled` | `bool` | `true` | Mirror logs to stderr |
| `logging.request_log_enabled` | `bool` | `true` | Log each request start/success/failure |
| `logging.slow_request_log_enabled` | `bool` | `true` | Log slow requests |
| `logging.slow_request_threshold_ms` | `u64` | `1000` | Slow request threshold |
| `logging.include_request_details_in_slow_log` | `bool` | `true` | Include request summary in slow logs |
| `logging.request_preview_chars` | `usize` | `160` | Max preview length for filter summaries |
| `logging.log_dir` | `string` | `""` | Custom log dir; falls back to `<db_path>/logs/` when empty |
| `logging.log_file_name` | `string` | `"vldb-lancedb.log"` | Base log file name (date auto-appended) |

### Path Rules

- Relative paths are resolved relative to the config file directory
- Values containing `://` are treated as URIs and used as-is
- Local directories are created automatically
- Logs are rotated daily as `vldb-lancedb_YYYY-MM-DD.log`

### Storage Backend Notes

- Local `db_path` directories are created automatically
- Plain `s3://` triggers a startup warning (unsafe for multi-writer)
- Use `s3+ddb://` for multi-instance or multi-writer deployments

---

## gRPC Service Reference

Service name: `vldb.lancedb.v1.LanceDbService`

All RPCs are **unary** (non-streaming).

### 1. CreateTable

Creates a new table with scalar and vector column definitions.

**Request**: `CreateTableRequest`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `table_name` | `string` | Yes | Table name |
| `columns` | `repeated ColumnDef` | Yes | Column definitions |
| `overwrite_if_exists` | `bool` | No | Overwrite if table exists |

**ColumnDef**:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `string` | Yes | Column name |
| `column_type` | `ColumnType` | Yes | Column type enum |
| `vector_dim` | `uint32` | Required for vector columns | Vector dimension (must be > 0) |
| `nullable` | `bool` | No | Whether the column can be null |

**ColumnType Enum**:

| Value | Description |
|-------|-------------|
| `COLUMN_TYPE_UNSPECIFIED` | Unspecified (invalid) |
| `COLUMN_TYPE_STRING` | String |
| `COLUMN_TYPE_INT32` | 32-bit signed integer |
| `COLUMN_TYPE_INT64` | 64-bit signed integer |
| `COLUMN_TYPE_UINT32` | 32-bit unsigned integer |
| `COLUMN_TYPE_UINT64` | 64-bit unsigned integer |
| `COLUMN_TYPE_FLOAT32` | 32-bit float |
| `COLUMN_TYPE_FLOAT64` | 64-bit float |
| `COLUMN_TYPE_BOOL` | Boolean |
| `COLUMN_TYPE_VECTOR_FLOAT32` | Fixed-size `float32` vector |

**Response**: `CreateTableResponse`

| Field | Type | Description |
|-------|------|-------------|
| `success` | `bool` | Whether the operation succeeded |
| `message` | `string` | Description message |

**Error Cases**:
- Empty `table_name`
- Empty `columns`
- Vector column with `vector_dim = 0`
- Table already exists and `overwrite_if_exists=false`

---

### 2. VectorUpsert

Appends or merges vector data into an existing table.

**Request**: `UpsertRequest`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `table_name` | `string` | Yes | Table name |
| `input_format` | `InputFormat` | No | Input data format |
| `data` | `bytes` | Yes | Data payload |
| `key_columns` | `repeated string` | No | Merge key column names |

**InputFormat Enum**:

| Value | Description |
|-------|-------------|
| `INPUT_FORMAT_UNSPECIFIED` | Default, treated as `JSON_ROWS` |
| `INPUT_FORMAT_JSON_ROWS` | JSON array, one object per row |
| `INPUT_FORMAT_ARROW_IPC` | Arrow IPC binary |

**Behavior**:

| `key_columns` | Behavior |
|---------------|----------|
| Empty | Append mode |
| Non-empty | Merge upsert — update on match, insert otherwise |

**JSON Ingestion Rules**:
- `data` must be a JSON array, e.g. `[{"id":1,"vec":[0.1,...]},{"id":2,...}]`
- Each row must be a JSON object
- Vector fields must be arrays of floats, e.g. `[0.1, 0.2, ..., 0.768]`
- Non-nullable fields must be present
- JSON field types must match the table schema

**Response**: `UpsertResponse`

| Field | Type | Description |
|-------|------|-------------|
| `success` | `bool` | Whether the operation succeeded |
| `message` | `string` | Description message |
| `version` | `uint64` | Table version after operation |
| `input_rows` | `uint64` | Number of input rows |
| `inserted_rows` | `uint64` | Number of inserted rows |
| `updated_rows` | `uint64` | Number of updated rows |
| `deleted_rows` | `uint64` | Number of deleted rows |

**Limits**:
- `data` size capped at **50 MB** (exceeding returns `INVALID_ARGUMENT`)

---

### 3. VectorSearch

Runs nearest-neighbor vector search.

**Request**: `SearchRequest`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `table_name` | `string` | Yes | Table name |
| `vector` | `repeated float` | Yes | Query vector |
| `limit` | `uint32` | No | Max rows to return; `0` defaults to 10 |
| `filter` | `string` | No | Additional predicate, e.g. `active = true` |
| `vector_column` | `string` | No | Named vector column for search |
| `output_format` | `OutputFormat` | No | Output data format |

**OutputFormat Enum**:

| Value | Description |
|-------|-------------|
| `OUTPUT_FORMAT_UNSPECIFIED` | Default, uses `ARROW_IPC` |
| `OUTPUT_FORMAT_ARROW_IPC` | Arrow IPC binary |
| `OUTPUT_FORMAT_JSON_ROWS` | JSON array |

**Response**: `SearchResponse`

| Field | Type | Description |
|-------|------|-------------|
| `success` | `bool` | Whether the operation succeeded |
| `message` | `string` | Description message |
| `format` | `string` | Actual format (`"json"` or `"arrow_ipc"`) |
| `rows` | `uint64` | Number of result rows |
| `data` | `bytes` | Result data |

**Limits**:
- `vector` must not be empty
- `limit` capped at **10,000** (client values above are silently truncated)
- JSON output may include a `_distance` field

**Error Cases**:
- Empty `table_name`
- Empty `vector`

---

### 4. Delete

Deletes rows matching a predicate.

**Request**: `DeleteRequest`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `table_name` | `string` | Yes | Table name |
| `condition` | `string` | Yes | Filter predicate |

**Predicate Examples**:
- `session_id = 'abc'`
- `id >= 100`
- `created_at < '2025-01-01'`

> The `condition` is passed directly to LanceDB as a filter expression. Callers must ensure the condition string is syntactically valid.

**Response**: `DeleteResponse`

| Field | Type | Description |
|-------|------|-------------|
| `success` | `bool` | Whether the operation succeeded |
| `message` | `string` | Description message |
| `version` | `uint64` | Table version after operation |
| `deleted_rows` | `uint64` | Number of deleted rows |

---

### 5. DropTable

Removes an entire LanceDB table.

**Request**: `DropTableRequest`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `table_name` | `string` | Yes | Table name |

**Response**: `DropTableResponse`

| Field | Type | Description |
|-------|------|-------------|
| `success` | `bool` | Whether the operation succeeded |
| `message` | `string` | Description message |

---

## Data Type Mapping

### JSON ↔ Arrow Mapping

| Arrow Type | JSON Type | Example |
|-----------|-----------|---------|
| `Utf8` | String | `"hello"` |
| `Int32` | Integer | `42` |
| `Int64` | Integer | `42` |
| `UInt32` | Integer | `42` |
| `UInt64` | Integer | `42` |
| `Float32` | Number | `3.14` |
| `Float64` | Number | `3.14` |
| `Boolean` | Boolean | `true` / `false` |
| `FixedSizeList(Float32, dim)` | Float array | `[0.1, 0.2, ..., 0.768]` |

### Ingestion (JSON → Arrow) Rules

- Vector columns (`FixedSizeList`): must be a JSON array with length equal to the declared dimension
- Nullable fields missing from a row are filled with `null`
- Non-nullable fields missing from a row cause an error
- Type mismatches cause an error

### Output (Arrow → JSON) Rules

- `FixedSizeList(Float32)`: encoded as a JSON array of floats
- Floats are output as `f64` precision in JSON (even `Float32` values)
- `null` values are output as JSON `null`
- Vector search results may include a `_distance` field

---

## Go Example Client

Generate Go stubs:

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

Run the example:

```bash
cd ./examples/go-client
go mod tidy
go run .
```

The example runs: `CreateTable` → `VectorUpsert` → `VectorSearch` → `Delete` → `DropTable`

---

## Current State & Known Issues

### Completed Features

- [x] 5 core RPCs (CreateTable / VectorUpsert / VectorSearch / Delete / DropTable)
- [x] Bidirectional JSON Rows and Arrow IPC encoding/decoding
- [x] per-table read/write concurrency coordination (RwLock)
- [x] Request timeout protection (configurable)
- [x] Global concurrency semaphore (configurable)
- [x] Payload size protection (50 MB cap)
- [x] Search limit cap (10,000 rows)
- [x] CPU-bound operations offloaded to blocking thread pool
- [x] Request logging, slow-request logging, daily log rotation
- [x] Docker support
- [x] S3 / S3+DynamoDB remote storage support

### Known Issues & Limitations

| Issue | Impact | Severity |
|-------|--------|----------|
| **No TLS/auth/authorization** | Network exposure risk | High |
| **Single-table writes fully serialized** | Same-table upserts cannot parallelize; per-table write QPS capped at ~60 | Medium |
| **JSON encode/decode precision loss** | Float32 output as f64 in JSON; Float64 input may be truncated to Float32 for vector columns | Low |
| **No request retry** | Network blips or transient LanceDB errors return directly to client | Medium |
| **No circuit breaker / fallback** | Downstream LanceDB failure causes requests to queue until timeout | Medium |
| **Global log Mutex contention** | >2000 QPS adds 2-5ms to p99 latency from log serialization | Low |
| **Single client can exhaust all permits** | No per-connection concurrency limit | Medium |
| **Plain `s3://` unsafe for multi-writer** | Concurrent writes from multiple instances may corrupt metadata | High (S3-only) |

### Completed Optimizations

| # | Optimization | Effect | Details |
|---|-------------|--------|---------|
| 1 | Shortened RwLock read scope | Write wait reduced 60-80% | [Plan](./concurrency_optimization_plan.md#task-1-shorten-rwlock-read-hold-scope-) |
| 2 | Offload CPU ops to spawn_blocking | Tokio workers no longer starved | [Plan](./concurrency_optimization_plan.md#task-2-offload-cpu-bound-operations-to-spawn_blocking-) |
| 3 | Explicit Tokio runtime | Parameters transparent and controllable | [Plan](./concurrency_optimization_plan.md#task-3-explicit-tokio-runtime-tuning-) |
| 4 | Payload & search limit caps | Prevents OOM | [Plan](./concurrency_optimization_plan.md#task-4-payload-size-and-search-limit-caps-) |
| 5 | Request timeout protection | p99 latency bounded | [Plan](./concurrency_optimization_plan.md#task-5-grpc-request-timeout-protection-) |
| 6 | Global concurrency semaphore | Prevents avalanche | [Plan](./concurrency_optimization_plan.md#task-6-global-concurrency-semaphore-) |

### Concurrency Estimates (8-core machine, after optimization)

| Scenario | Est. QPS | Notes |
|----------|---------|-------|
| Read-only (small results, limit ≤ 10) | 800-3000 | Bottleneck: Tokio workers + disk I/O |
| Read-only (large results, limit ≤ 1000) | 300-800 | Encoding in blocking pool, no worker starvation |
| Read-only (max results, limit ≤ 10000) | 100-300 | Protected by MAX_SEARCH_LIMIT |
| Single-table write (small batch, 100 rows) | 25-60 | Write-locked; limited by LanceDB write speed |
| Mixed read/write (9:1) | 600-2500 | Read-heavy; writes briefly block |
| Mixed read/write (5:5) | 300-1000 | Read-write contention increases |
| Mixed read/write (1:9) | 30-120 | Write-heavy; same-table serialization |

> Detailed analysis: [docs/concurrency_optimization_plan.md](./concurrency_optimization_plan.md)

---

## Integration Notes

### Security

- The service has **no built-in** TLS, authentication, or authorization
- Use a reverse proxy (Nginx/Envoy) or network isolation for security
- Default listen address is `127.0.0.1`; configure `0.0.0.0` explicitly for external access

### Performance

- Writes to the same table are fully serialized; writes to different tables are fully parallel
- Distribute high-frequency writes across different tables to increase throughput
- Large-result-set search encoding consumes blocking pool threads
- Request timeout defaults to 30 seconds; slow queries are terminated

### Protocol

- All RPCs are unary; server streaming and bidi streaming are not supported
- `UpsertRequest.data` capped at **50 MB**; exceeding returns `INVALID_ARGUMENT`
- `SearchRequest.limit` capped at **10,000**; higher values are silently truncated
- `Delete.condition` is passed verbatim to LanceDB; callers must ensure valid syntax
- Table names are automatically `trim()`med (leading/trailing whitespace removed)

### Operations

- Request logging and slow-request logging are enabled by default
- Logs are written to `<db_path>/logs/`, rotated daily
- `protoc` is required at build time
- Rust `1.94.0` is required; older versions will fail to compile
- Use `s3+ddb://` for multi-instance deployments

### Caller Example (Pseudocode)

```protobuf
// 1. Create a table with a vector column
CreateTableRequest {
  table_name: "demo"
  columns: [
    { name: "id", column_type: COLUMN_TYPE_INT64, nullable: false },
    { name: "embedding", column_type: COLUMN_TYPE_VECTOR_FLOAT32, vector_dim: 768 }
  ]
}

// 2. Ingest JSON data
UpsertRequest {
  table_name: "demo"
  input_format: INPUT_FORMAT_JSON_ROWS
  data: '[{"id":1,"embedding":[0.1,0.2,...,0.768]},{"id":2,"embedding":[...]}]'
}

// 3. Vector search
SearchRequest {
  table_name: "demo"
  vector: [0.1, 0.2, ..., 0.768]
  limit: 10
  output_format: OUTPUT_FORMAT_JSON_ROWS
}

// 4. Conditional delete
DeleteRequest {
  table_name: "demo"
  condition: "id = 1"
}

// 5. Drop table
DropTableRequest {
  table_name: "demo"
}
```
