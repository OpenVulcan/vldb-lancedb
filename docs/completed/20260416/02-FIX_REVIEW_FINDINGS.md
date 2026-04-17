## 任务目标

修复完整项目审查中识别出的三个问题，确保 FFI 字节缓冲区释放安全、多库命名路径不会逃逸 `db_root`、以及纯 Rust API 的检索结果格式元信息与实际编码保持一致。

## 执行步骤

1. 审查 `ffi.rs`、`manager.rs`、`engine.rs` 与 C 头文件，确认问题修复所影响的公开接口与测试范围。
2. 调整 FFI 字节缓冲区结构与释放协议，确保分配和释放使用一致的容量信息。
3. 为命名数据库增加输入约束与路径边界保护，阻止绝对路径、目录穿越和分隔符注入。
4. 修正 `vector_search` 的返回格式字段，使其反映实际输出编码格式。
5. 补充或更新单元测试，覆盖上述三类问题的回归场景。
6. 运行相关测试验证修复结果，并整理执行总结后归档计划文件。

## 技术选型

- 使用最小侵入方式修复现有实现，优先保持模块职责不变。
- 对 FFI 问题采用显式记录容量的方式，避免依赖分配器行为或隐式收缩语义。
- 对数据库名校验采用“白名单式约束”，从输入层直接拒绝危险名称。
- 使用单元测试验证返回契约、路径解析约束和 FFI 缓冲区协议。

## 验收标准

- `vldb_lancedb_bytes_free` 能与导出缓冲区的真实分配信息匹配，不再依赖错误容量。
- 命名数据库无法通过绝对路径、`..` 或路径分隔符逃逸 `db_root`。
- `vector_search` 在 `Unspecified` 输入下返回与真实编码一致的 `format`。
- 新增或更新的测试通过，且现有相关测试不回归。

## 执行变更总结

### 1. 核心修复与调整概述

- 修复了 FFI 原始字节缓冲区释放协议，导出缓冲区现在显式保留真实容量，避免释放时使用错误布局导致的内存安全风险。
- 为命名数据库路径增加单层名称校验，拒绝路径分隔符、目录穿越等可逃逸 `db_root` 的输入。
- 修正纯 Rust `vector_search` 的返回格式语义，未指定输出格式时现在会返回实际采用的 `ArrowIpc` 编码标识。
- 同步更新了公开头文件、FFI 使用文档和回归测试，保证对外契约与实现一致。

### 2. 📂文件变更清单

- 修改：`src/ffi.rs`
- 修改：`src/manager.rs`
- 修改：`src/engine.rs`
- 修改：`include/vldb_lancedb.h`
- 修改：`docs/LIBRARY_USAGE.zh-CN.md`
- 新增：`docs/plan/20260416-02-FIX_REVIEW_FINDINGS.md`（待归档）

### 3. 💻关键代码调整详情

- `src/ffi.rs`
- 为 `VldbLancedbByteBuffer` 新增 `cap` 字段，并让 `allocate_byte_buffer` / `vldb_lancedb_bytes_free` 使用一致的长度与容量信息。
- 新增 FFI 单元测试，验证保留额外容量时仍可安全释放。
- `src/manager.rs`
- 新增 `validate_database_name`，在命名数据库路径解析前拒绝包含路径分隔符或非单层路径片段的库名。
- 新增回归测试覆盖目录穿越与嵌套路径输入。
- `src/engine.rs`
- 新增 `normalize_output_format`，统一将 `Unspecified` 归一化为真实输出格式 `ArrowIpc`。
- 新增对应单元测试，覆盖默认输出格式归一化行为。
- `include/vldb_lancedb.h` 与 `docs/LIBRARY_USAGE.zh-CN.md`
- 同步公开结构体定义与释放规则说明，避免接入方继续按旧字段布局使用。

### 4. ⚠️遗留问题与注意事项

- 本次修复调整了 `VldbLancedbByteBuffer` 的 ABI 结构，所有使用该头文件的 FFI 调用方都需要同步更新到新版本头文件。
- 当前工作区存在本次任务之外的已有修改（如 `src/config.rs`、`src/main.rs`、`src/runtime.rs`、`src/service.rs`），本次未对其进行回退或改写。
- 已执行 `cargo fmt --all` 与 `cargo test`，当前相关测试全部通过。
