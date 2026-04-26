# vldb-lancedb

`vldb-lancedb` is a Rust LanceDB vector data gateway that can be used as a gRPC service, an embeddable Rust library, or a C-compatible FFI dynamic library. It is part of the OpenVulcan local data gateway stack and focuses on multi-language access to local vector storage.

The crate provides table creation, vector upsert, vector search, delete, and drop-table operations with JSON Rows and Arrow IPC payload support.

## Features

- LanceDB gRPC gateway built with `tonic`
- Embeddable Rust library API for local vector workloads
- C ABI / FFI exports for Go, C, Lua, and other native runtimes
- JSON Rows and Arrow IPC input support
- JSON Rows and Arrow IPC search output support
- Per-table concurrency coordination
- Request timeout, payload limit, and concurrency limit controls
- Multi-database runtime support for local gateway hosts

## Installation

Add the crate to a Rust project:

```bash
cargo add vldb-lancedb
```

Or add it manually:

```toml
[dependencies]
vldb-lancedb = "0.1.5"
```

## Run the Gateway

Build and run the gRPC service from this repository:

```bash
cargo run --release -- --config ./vldb-lancedb.json
```

The default gRPC endpoint is:

```text
127.0.0.1:19301
```

Use `vldb-lancedb.json.example` as the starting point for a custom service configuration.

## Rust Library Usage

The library mode exposes the same LanceDB execution core used by the gRPC service. Typical callers create a runtime, open the default or a named database engine, and then call the typed create/upsert/search/delete/drop operations.

Important modules:

- `runtime`: embedded LanceDB runtime
- `manager`: default and named database manager
- `engine`: core LanceDB operations
- `types`: typed library input and output models
- `ffi`: exported FFI symbols and FFI-safe types

## FFI Usage

The crate also builds a `cdylib` and ships the C header in `include/vldb_lancedb.h`. Native hosts should prefer the non-JSON FFI entry points for hot vector upsert/search paths and keep JSON FFI calls for compatibility, scripting, and diagnostics.

Build the dynamic library:

```bash
cargo build --release
```

The platform-specific shared library is emitted under `target/release`.

## Documentation

- [English guide](https://github.com/OpenVulcan/vldb-lancedb/blob/main/docs/README.en.md)
- [Chinese guide](https://github.com/OpenVulcan/vldb-lancedb/blob/main/docs/README.zh-CN.md)
- [Library and FFI guide](https://github.com/OpenVulcan/vldb-lancedb/blob/main/docs/LIBRARY_USAGE.zh-CN.md)
- [gRPC protobuf definition](https://github.com/OpenVulcan/vldb-lancedb/blob/main/proto/v1/lancedb.proto)
- [Go gRPC example](https://github.com/OpenVulcan/vldb-lancedb/tree/main/examples/go-client)
- [Go FFI example](https://github.com/OpenVulcan/vldb-lancedb/tree/main/examples/go-ffi)
- [API documentation](https://docs.rs/vldb-lancedb)

## Package Contents

The crates.io package intentionally includes only the files required for Rust builds, generated documentation, protobuf compilation, FFI headers, and user-facing crate documentation. CI, Docker packaging, local runtime data, and release automation files are kept out of the published crate.

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).
