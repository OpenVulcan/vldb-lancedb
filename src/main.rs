mod config;
mod logging;
mod service;

pub mod pb {
    tonic::include_proto!("vldb.lancedb.v1");
}

use service::{LanceDbGrpcService, ServiceConfig};
use std::time::Duration;
use tokio::net::lookup_host;
use tonic::transport::Server;
use vldb_lancedb::engine::LanceDbEngineOptions;
use vldb_lancedb::runtime::LanceDbRuntime;

use crate::config::{BoxError, load};
use crate::logging::ServiceLogger;
use crate::pb::lance_db_service_server::LanceDbServiceServer;

fn main() -> Result<(), BoxError> {
    let worker_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .max_blocking_threads(128)
        .thread_keep_alive(Duration::from_secs(60))
        .enable_all()
        .build()?;

    rt.block_on(async_main())
}

async fn async_main() -> Result<(), BoxError> {
    let cfg = load()?;
    let logger = ServiceLogger::new("vldb-lancedb", cfg.logging())?;
    let runtime = LanceDbRuntime::new(
        cfg.database_runtime_config(),
        LanceDbEngineOptions {
            max_concurrent_requests: cfg.max_concurrent_requests,
            ..LanceDbEngineOptions::default()
        },
    );

    if let Some(warning) = cfg.runtime.concurrent_write_warning() {
        eprintln!("warning: {warning}");
        logger.log("warning", warning);
    }
    let engine = runtime.open_default_engine().await?;

    let bind_text = format!("{}:{}", cfg.host, cfg.port);
    let mut addrs = lookup_host(bind_text.as_str()).await?;
    let addr = addrs
        .next()
        .ok_or_else(|| format!("failed to resolve bind address: {bind_text}"))?;

    if let Some(source) = cfg.source.as_ref() {
        println!("using config file: {}", source.display());
    } else {
        println!("using default config");
    }
    println!("lancedb uri: {}", cfg.default_db_path());
    match cfg.runtime.read_consistency_interval_ms {
        Some(0) => println!("lancedb read consistency interval: 0 ms (strong)"),
        Some(ms) => println!("lancedb read consistency interval: {} ms", ms),
        None => println!("lancedb read consistency interval: disabled"),
    }
    if let Some(log_path) = logger.log_path() {
        println!("request log file: {}", log_path.display());
    } else if cfg.logging().enabled {
        println!("request log file: disabled");
    }
    println!("grpc listen: {}", addr);
    println!(
        "request timeout: {}",
        cfg.grpc_request_timeout_ms
            .map(|ms| format!("{} ms", ms))
            .unwrap_or_else(|| "disabled".to_string())
    );
    println!("max concurrent requests: {}", cfg.max_concurrent_requests);

    let service_config = ServiceConfig {
        request_timeout: cfg.grpc_request_timeout_ms.map(Duration::from_millis),
    };

    Server::builder()
        .add_service(LanceDbServiceServer::new(LanceDbGrpcService::from_engine(
            engine,
            logger,
            service_config,
        )))
        .serve(addr)
        .await?;

    Ok(())
}
