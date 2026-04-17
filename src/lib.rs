/// 数据库管理模块，负责默认库与多库连接管理。
/// Database management module responsible for default-database and multi-database connection management.
pub mod manager;

/// 类型模块，负责定义纯 Rust 核心输入输出结构。
/// Types module responsible for defining pure Rust core input and output structures.
pub mod types;

/// 引擎模块，负责提供可嵌入的 LanceDB 核心操作能力。
/// Engine module responsible for providing embeddable LanceDB core operations.
pub mod engine;

/// 运行时模块，负责组合数据库管理器与引擎实例化流程。
/// Runtime module responsible for composing the database manager with engine instantiation flows.
pub mod runtime;

/// FFI 模块，负责导出稳定的 C ABI 供非 Rust 调用方使用。
/// FFI module responsible for exporting a stable C ABI for non-Rust callers.
pub mod ffi;
