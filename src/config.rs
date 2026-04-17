use crate::logging::LoggingConfig;
use serde::Deserialize;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use vldb_lancedb::manager::DatabaseRuntimeConfig;

const DEFAULT_CONFIG_FILE: &str = "vldb-lancedb.json";
const LEGACY_CONFIG_FILE: &str = "lancedb.json";

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub db_path: String,
    pub read_consistency_interval_ms: Option<u64>,
    pub grpc_request_timeout_ms: Option<u64>,
    pub max_concurrent_requests: usize,
    pub logging: LoggingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 19301,
            db_path: "./data".to_string(),
            read_consistency_interval_ms: Some(0),
            grpc_request_timeout_ms: Some(30_000),
            max_concurrent_requests: 500,
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub host: String,
    pub port: u16,
    pub grpc_request_timeout_ms: Option<u64>,
    pub max_concurrent_requests: usize,
    pub source: Option<PathBuf>,
    pub logging: LoggingConfig,
    pub runtime: DatabaseRuntimeConfig,
}

impl ResolvedConfig {
    /// 返回默认数据库路径。
    /// Return the default database path.
    pub fn default_db_path(&self) -> &str {
        &self.runtime.default_db_path
    }

    /// 返回日志配置。
    /// Return the logging configuration.
    pub fn logging(&self) -> &LoggingConfig {
        &self.logging
    }

    /// 返回数据库运行时配置。
    /// Return the database runtime configuration.
    pub fn database_runtime_config(&self) -> DatabaseRuntimeConfig {
        self.runtime.clone()
    }
}

pub fn load() -> Result<ResolvedConfig, BoxError> {
    let explicit_config = parse_config_arg()?;
    let config_path = explicit_config.or(find_default_config_file()?);

    let (mut config, source) = match config_path {
        Some(path) => {
            let content = fs::read_to_string(&path)?;
            let config: Config = serde_json::from_str(&content)?;
            (config, Some(path))
        }
        None => (Config::default(), None),
    };

    let base_dir = source
        .as_ref()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or(env::current_dir()?);

    let resolved_db_path = resolve_data_path(&config.db_path, &base_dir);
    config.logging.log_dir = resolve_log_dir(&config.logging.log_dir, &resolved_db_path, &base_dir);
    validate_config(&config)?;

    Ok(ResolvedConfig {
        host: config.host,
        port: config.port,
        grpc_request_timeout_ms: config.grpc_request_timeout_ms,
        max_concurrent_requests: config.max_concurrent_requests,
        source,
        logging: config.logging,
        runtime: DatabaseRuntimeConfig {
            default_db_path: resolved_db_path,
            db_root: None,
            read_consistency_interval_ms: config.read_consistency_interval_ms,
        },
    })
}

fn validate_config(config: &Config) -> Result<(), BoxError> {
    if config.host.trim().is_empty() {
        return Err(invalid_input("config.host must not be empty"));
    }
    if config.port == 0 {
        return Err(invalid_input("config.port must be greater than 0"));
    }
    if config.db_path.trim().is_empty() {
        return Err(invalid_input("config.db_path must not be empty"));
    }
    if config.logging.request_preview_chars == 0 {
        return Err(invalid_input(
            "config.logging.request_preview_chars must be greater than 0",
        ));
    }
    if config.logging.slow_request_threshold_ms == 0 {
        return Err(invalid_input(
            "config.logging.slow_request_threshold_ms must be greater than 0",
        ));
    }
    if config.logging.file_enabled && config.logging.log_file_name.trim().is_empty() {
        return Err(invalid_input(
            "config.logging.log_file_name must not be empty when file logging is enabled",
        ));
    }
    Ok(())
}

fn parse_config_arg() -> Result<Option<PathBuf>, BoxError> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-config" || arg == "--config" {
            let next = args
                .next()
                .ok_or_else(|| invalid_input("missing value after -config / --config"))?;
            let current_dir = env::current_dir()?;
            return Ok(Some(resolve_path_like_shell(&next, &current_dir)));
        }
    }
    Ok(None)
}

fn find_default_config_file() -> Result<Option<PathBuf>, BoxError> {
    let mut candidates = Vec::new();

    if let Ok(current_exe) = env::current_exe()
        && let Some(exe_dir) = current_exe.parent()
    {
        candidates.push(exe_dir.join(DEFAULT_CONFIG_FILE));
        candidates.push(exe_dir.join(LEGACY_CONFIG_FILE));
    }

    let current_dir = env::current_dir()?;
    candidates.push(current_dir.join(DEFAULT_CONFIG_FILE));
    candidates.push(current_dir.join(LEGACY_CONFIG_FILE));

    Ok(candidates.into_iter().find(|p| p.is_file()))
}

fn resolve_data_path(raw: &str, base_dir: &Path) -> String {
    if looks_like_uri(raw) {
        return raw.to_string();
    }

    resolve_path_like_shell(raw, base_dir)
        .to_string_lossy()
        .to_string()
}

fn resolve_log_dir(configured_dir: &Path, db_path: &str, base_dir: &Path) -> PathBuf {
    if !configured_dir.as_os_str().is_empty() {
        return resolve_path_like_shell(&configured_dir.to_string_lossy(), base_dir);
    }

    if looks_like_uri(db_path) {
        return base_dir.join("vldb-lancedb-logs");
    }

    let db_path = PathBuf::from(db_path);
    db_path.join("logs")
}

fn resolve_path_like_shell(raw: &str, base_dir: &Path) -> PathBuf {
    let expanded = expand_tilde(raw);
    let path = PathBuf::from(expanded);

    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn expand_tilde(raw: &str) -> String {
    if raw == "~" {
        return home_dir_string().unwrap_or_else(|| raw.to_string());
    }

    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = home_dir_string()
    {
        return PathBuf::from(home).join(rest).to_string_lossy().to_string();
    }

    if let Some(rest) = raw.strip_prefix("~\\")
        && let Some(home) = home_dir_string()
    {
        return PathBuf::from(home).join(rest).to_string_lossy().to_string();
    }

    raw.to_string()
}

fn home_dir_string() -> Option<String> {
    env::var("HOME")
        .ok()
        .or_else(|| env::var("USERPROFILE").ok())
}

fn looks_like_uri(value: &str) -> bool {
    value.contains("://")
}

fn invalid_input(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Config, ResolvedConfig, resolve_log_dir};
    use crate::logging::LoggingConfig;
    use std::path::{Path, PathBuf};
    use vldb_lancedb::manager::DatabaseRuntimeConfig;

    #[test]
    fn default_log_dir_uses_logs_subdir_for_local_db_path() {
        let resolved = resolve_log_dir(Path::new(""), "/srv/vldb/lancedb", Path::new("/etc/vldb"));
        assert_eq!(resolved, PathBuf::from("/srv/vldb/lancedb/logs"));
    }

    #[test]
    fn explicit_relative_log_dir_is_resolved_from_config_dir() {
        let resolved = resolve_log_dir(
            Path::new("./logs"),
            "/srv/vldb/lancedb",
            Path::new("/etc/vldb"),
        );
        assert_eq!(resolved, PathBuf::from("/etc/vldb/logs"));
    }

    #[test]
    fn uri_db_path_falls_back_to_config_relative_log_dir() {
        let resolved =
            resolve_log_dir(Path::new(""), "s3://bucket/lancedb", Path::new("/etc/vldb"));
        assert_eq!(resolved, PathBuf::from("/etc/vldb/vldb-lancedb-logs"));
    }

    #[test]
    fn default_read_consistency_interval_is_strong() {
        assert_eq!(Config::default().read_consistency_interval_ms, Some(0));
    }

    #[test]
    fn plain_s3_uri_emits_concurrent_write_warning() {
        let cfg = ResolvedConfig {
            host: "127.0.0.1".to_string(),
            port: 19301,
            grpc_request_timeout_ms: Some(30_000),
            max_concurrent_requests: 500,
            source: None,
            logging: LoggingConfig::default(),
            runtime: DatabaseRuntimeConfig {
                default_db_path: "s3://bucket/lancedb".to_string(),
                db_root: None,
                read_consistency_interval_ms: Some(0),
            },
        };

        assert_eq!(cfg.runtime.read_consistency_interval_ms, Some(0));
        assert!(cfg.runtime.concurrent_write_warning().is_some());
    }

    #[test]
    fn s3_ddb_uri_does_not_emit_concurrent_write_warning() {
        let cfg = ResolvedConfig {
            host: "127.0.0.1".to_string(),
            port: 19301,
            grpc_request_timeout_ms: Some(30_000),
            max_concurrent_requests: 500,
            source: None,
            logging: LoggingConfig::default(),
            runtime: DatabaseRuntimeConfig {
                default_db_path: "s3+ddb://bucket/lancedb".to_string(),
                db_root: None,
                read_consistency_interval_ms: Some(250),
            },
        };

        assert_eq!(cfg.runtime.read_consistency_interval_ms, Some(250));
        assert!(cfg.runtime.concurrent_write_warning().is_none());
    }
}
