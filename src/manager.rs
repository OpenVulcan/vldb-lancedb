use lancedb::Connection;
use std::collections::HashMap;
use std::error::Error;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// 通用错误类型，供库层数据库管理模块复用。
/// Shared error type reused by the library-side database management module.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// 数据库运行时配置，仅保留库层真正需要的 LanceDB 运行信息。
/// Database runtime configuration that keeps only the LanceDB runtime information truly needed by the library layer.
#[derive(Debug, Clone)]
pub struct DatabaseRuntimeConfig {
    pub default_db_path: String,
    pub db_root: Option<String>,
    pub read_consistency_interval_ms: Option<u64>,
}

impl DatabaseRuntimeConfig {
    /// 判断默认数据库是否为本地路径。
    /// Determine whether the default database points to a local path.
    pub fn is_local_default_db_path(&self) -> bool {
        !looks_like_uri(&self.default_db_path)
    }

    /// 返回读一致性配置对应的持续时间。
    /// Return the read consistency interval as a duration when configured.
    pub fn read_consistency_interval(&self) -> Option<std::time::Duration> {
        self.read_consistency_interval_ms
            .map(std::time::Duration::from_millis)
    }

    /// 如果默认库使用 plain s3 路径，则返回并发写风险提示。
    /// Return the concurrent-write warning when the default database uses a plain s3 path.
    pub fn concurrent_write_warning(&self) -> Option<&'static str> {
        if is_plain_s3_uri(&self.default_db_path) {
            Some(
                "plain s3:// storage is not safe for concurrent LanceDB writers; use s3+ddb:// for multi-writer deployments or ensure this service is the only writer for each table",
            )
        } else {
            None
        }
    }

    /// 按名称解析数据库目标路径；空名称回落为默认库。
    /// Resolve the target database path by name; blank names fall back to the default database.
    pub fn database_path_for_name(&self, database_name: Option<&str>) -> Result<String, BoxError> {
        let normalized = database_name
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let Some(database_name) = normalized else {
            return Ok(self.default_db_path.clone());
        };
        validate_database_name(database_name)?;

        let Some(db_root) = self.db_root.as_ref() else {
            return Err(invalid_input(format!(
                "database `{database_name}` requires a database root to be configured"
            )));
        };

        Ok(join_database_root(db_root, database_name))
    }
}

/// 数据库连接管理器，负责默认库与命名子库的惰性打开和缓存。
/// Database connection manager responsible for lazily opening and caching the default database and named child databases.
#[derive(Clone)]
pub struct DatabaseManager {
    config: Arc<DatabaseRuntimeConfig>,
    connections: Arc<Mutex<HashMap<String, Arc<Connection>>>>,
}

impl DatabaseManager {
    /// 基于解析后的配置创建数据库管理器。
    /// Create a database manager from resolved configuration.
    pub fn new(config: DatabaseRuntimeConfig) -> Self {
        Self {
            config: Arc::new(config),
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 返回共享配置引用，便于上层继续复用公共配置。
    /// Return the shared configuration reference so upper layers can continue reusing common settings.
    pub fn config(&self) -> &DatabaseRuntimeConfig {
        self.config.as_ref()
    }

    /// 打开或获取默认数据库连接。
    /// Open or retrieve the default database connection.
    pub async fn open_default(&self) -> Result<Arc<Connection>, BoxError> {
        self.open_named(None).await
    }

    /// 按名称打开或获取数据库连接；`None` 表示默认库。
    /// Open or retrieve a database connection by name; `None` means the default database.
    pub async fn open_named(
        &self,
        database_name: Option<&str>,
    ) -> Result<Arc<Connection>, BoxError> {
        let database_key = normalize_database_key(database_name);
        let target_path = self.config.database_path_for_name(database_name)?;

        {
            let guard = self.connections.lock().await;
            if let Some(existing) = guard.get(&database_key) {
                return Ok(Arc::clone(existing));
            }
        }

        if !target_path.contains("://") {
            tokio::fs::create_dir_all(&target_path).await?;
        }

        let mut builder = lancedb::connect(&target_path);
        if let Some(interval) = self.config.read_consistency_interval() {
            builder = builder.read_consistency_interval(interval);
        }
        let connection = Arc::new(builder.execute().await?);

        let mut guard = self.connections.lock().await;
        let entry = guard
            .entry(database_key)
            .or_insert_with(|| Arc::clone(&connection));
        Ok(Arc::clone(entry))
    }

    /// 解析某个库名对应的实际路径，不触发连接创建。
    /// Resolve the concrete path for a database name without opening a connection.
    pub fn database_path_for_name(&self, database_name: Option<&str>) -> Result<String, BoxError> {
        self.config.database_path_for_name(database_name)
    }

    /// 返回当前已缓存的库键名列表，用于诊断或管理场景。
    /// Return the cached database keys for diagnostics or management scenarios.
    pub async fn cached_database_keys(&self) -> Vec<String> {
        let guard = self.connections.lock().await;
        let mut keys = guard.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        keys
    }
}

/// 将外部输入的库名统一规整成内部缓存键。
/// Normalize an external database name into the internal cache key.
fn normalize_database_key(database_name: Option<&str>) -> String {
    database_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string()
}

/// 判断给定路径是否为 URI 风格。
/// Determine whether the provided path is URI-like.
fn looks_like_uri(value: &str) -> bool {
    value.contains("://")
}

/// 判断给定路径是否为 plain s3 URI。
/// Determine whether the provided path is a plain s3 URI.
fn is_plain_s3_uri(value: &str) -> bool {
    value.to_ascii_lowercase().starts_with("s3://")
}

/// 统一拼接多库根目录与子库名称。
/// Join the multi-database root with a child database name consistently.
fn join_database_root(db_root: &str, database_name: &str) -> String {
    if looks_like_uri(db_root) {
        let trimmed = db_root.trim_end_matches('/');
        return format!("{trimmed}/{database_name}");
    }

    PathBuf::from(db_root)
        .join(database_name)
        .to_string_lossy()
        .to_string()
}

/// 校验命名数据库名称是否安全且限定为单层名称。
/// Validate that a named database identifier is safe and restricted to a single segment.
fn validate_database_name(database_name: &str) -> Result<(), BoxError> {
    if database_name.contains('/') || database_name.contains('\\') {
        return Err(invalid_input(format!(
            "database `{database_name}` must not contain path separators"
        )));
    }

    let mut components = Path::new(database_name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(invalid_input(format!(
            "database `{database_name}` must be a single path segment"
        ))),
    }
}

/// 构造输入错误，供库层统一返回。
/// Build an invalid-input error reused by the library layer.
fn invalid_input(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{DatabaseManager, DatabaseRuntimeConfig};

    fn sample_config() -> DatabaseRuntimeConfig {
        DatabaseRuntimeConfig {
            default_db_path: "/srv/vldb/default".to_string(),
            db_root: Some("/srv/vldb/databases".to_string()),
            read_consistency_interval_ms: Some(0),
        }
    }

    #[test]
    fn default_database_path_uses_default_db_path() {
        let manager = DatabaseManager::new(sample_config());
        assert_eq!(
            manager
                .database_path_for_name(None)
                .expect("default database path should resolve"),
            "/srv/vldb/default"
        );
    }

    #[test]
    fn named_database_path_uses_database_root() {
        let manager = DatabaseManager::new(sample_config());
        assert_eq!(
            manager
                .database_path_for_name(Some("memory"))
                .expect("named database path should resolve"),
            std::path::PathBuf::from("/srv/vldb/databases")
                .join("memory")
                .to_string_lossy()
                .to_string()
        );
    }

    #[test]
    fn named_database_path_rejects_traversal_segments() {
        let manager = DatabaseManager::new(sample_config());
        let error = manager
            .database_path_for_name(Some("../escape"))
            .expect_err("traversal name should be rejected");
        assert!(error.to_string().contains("path separators"));
    }

    #[test]
    fn named_database_path_rejects_path_separators() {
        let manager = DatabaseManager::new(sample_config());
        let error = manager
            .database_path_for_name(Some("nested/name"))
            .expect_err("nested path should be rejected");
        assert!(error.to_string().contains("path separators"));
    }
}
