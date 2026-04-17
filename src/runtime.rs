use crate::engine::{LanceDbEngine, LanceDbEngineOptions};
use crate::manager::{BoxError, DatabaseManager, DatabaseRuntimeConfig};

/// 嵌入式 LanceDB 运行时，负责为调用方统一管理数据库实例与引擎构造。
/// Embedded LanceDB runtime responsible for managing database instances and engine construction for callers.
#[derive(Clone)]
pub struct LanceDbRuntime {
    manager: DatabaseManager,
    engine_options: LanceDbEngineOptions,
}

impl LanceDbRuntime {
    /// 基于运行时数据库配置与引擎选项创建嵌入式运行时。
    /// Create the embedded runtime from database runtime configuration and engine options.
    pub fn new(config: DatabaseRuntimeConfig, engine_options: LanceDbEngineOptions) -> Self {
        Self {
            manager: DatabaseManager::new(config),
            engine_options,
        }
    }

    /// 返回共享数据库管理器，供上层复用路径解析或连接缓存能力。
    /// Return the shared database manager so upper layers can reuse path resolution or connection caching.
    pub fn manager(&self) -> &DatabaseManager {
        &self.manager
    }

    /// 返回当前运行时默认采用的引擎选项。
    /// Return the engine options used by this runtime.
    pub fn engine_options(&self) -> &LanceDbEngineOptions {
        &self.engine_options
    }

    /// 打开默认数据库并构造其对应的引擎实例。
    /// Open the default database and construct its engine instance.
    pub async fn open_default_engine(&self) -> Result<LanceDbEngine, BoxError> {
        self.open_named_engine(None).await
    }

    /// 按名称打开数据库并构造对应引擎；`None` 表示默认库。
    /// Open a named database and construct its engine; `None` means the default database.
    pub async fn open_named_engine(
        &self,
        database_name: Option<&str>,
    ) -> Result<LanceDbEngine, BoxError> {
        let connection = self.manager.open_named(database_name).await?;
        Ok(LanceDbEngine::new(connection, self.engine_options.clone()))
    }

    /// 解析某个数据库名称对应的实际路径，不触发连接创建。
    /// Resolve the concrete path for a database name without opening the connection.
    pub fn database_path_for_name(&self, database_name: Option<&str>) -> Result<String, BoxError> {
        self.manager.database_path_for_name(database_name)
    }

    /// 返回当前已经缓存的数据库键列表。
    /// Return the cached database keys currently tracked by the runtime.
    pub async fn cached_database_keys(&self) -> Vec<String> {
        self.manager.cached_database_keys().await
    }
}

#[cfg(test)]
mod tests {
    use super::LanceDbRuntime;
    use crate::engine::LanceDbEngineOptions;
    use crate::manager::DatabaseRuntimeConfig;

    fn sample_runtime() -> LanceDbRuntime {
        LanceDbRuntime::new(
            DatabaseRuntimeConfig {
                default_db_path: "/srv/vldb/default".to_string(),
                db_root: Some("/srv/vldb/databases".to_string()),
                read_consistency_interval_ms: Some(250),
            },
            LanceDbEngineOptions {
                max_upsert_payload: 1024,
                max_search_limit: 64,
                max_concurrent_requests: 8,
            },
        )
    }

    #[test]
    fn runtime_exposes_engine_options() {
        let runtime = sample_runtime();
        assert_eq!(runtime.engine_options().max_upsert_payload, 1024);
        assert_eq!(runtime.engine_options().max_search_limit, 64);
        assert_eq!(runtime.engine_options().max_concurrent_requests, 8);
    }

    #[test]
    fn runtime_resolves_named_database_path_via_manager() {
        let runtime = sample_runtime();
        assert_eq!(
            runtime
                .database_path_for_name(Some("memory"))
                .expect("named database path should resolve"),
            std::path::PathBuf::from("/srv/vldb/databases")
                .join("memory")
                .to_string_lossy()
                .to_string()
        );
    }
}
