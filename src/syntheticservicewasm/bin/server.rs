use anyhow::{Context, Result};
use redis::AsyncCommands;
use std::sync::Arc;
use tonic::{transport::Server, Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

#[path = "../src/core.rs"]
mod core;

use core::{DbClient, NetworkClient, WorkloadConfig};
use hipstershop::product_catalog_service_client::ProductCatalogServiceClient;
use hipstershop::synthetic_service_server::{SyntheticService, SyntheticServiceServer};
use hipstershop::{Empty, WorkloadRequest, WorkloadResponse};

// ---------------------------------------------------------------------------
// Native network adapter — uses a tonic gRPC client
// ---------------------------------------------------------------------------

struct TonicNetworkClient {
    addr: String,
}

#[tonic::async_trait]
impl NetworkClient for TonicNetworkClient {
    async fn list_products_once(&self) -> Result<usize, String> {
        let endpoint =
            tonic::transport::Endpoint::from_shared(format!("http://{}", self.addr))
                .map_err(|e| e.to_string())?;
        let channel = endpoint.connect().await.map_err(|e| e.to_string())?;
        let mut client = ProductCatalogServiceClient::new(channel);
        let resp = client
            .list_products(Empty {})
            .await
            .map_err(|e| e.to_string())?;
        Ok(resp.into_inner().products.len())
    }
}

// ---------------------------------------------------------------------------
// Native DB adapter — uses the redis crate
// ---------------------------------------------------------------------------

struct NativeDbClient {
    client: redis::Client,
}

#[tonic::async_trait]
impl DbClient for NativeDbClient {
    async fn roundtrip(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| e.to_string())?;
        conn.set::<_, _, ()>(key, value)
            .await
            .map_err(|e| e.to_string())?;
        let _: Option<Vec<u8>> = conn.get(key).await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// gRPC service — delegates entirely to core logic
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SyntheticServiceImpl {
    catalog_addr: String,
    redis_client: Arc<redis::Client>,
}

#[tonic::async_trait]
impl SyntheticService for SyntheticServiceImpl {
    async fn run_workload(
        &self,
        _request: Request<WorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        let config = WorkloadConfig::from_env();
        // log config for debugging
        println!("Running workload with config: {:?}", config);
        let net = TonicNetworkClient {
            addr: self.catalog_addr.clone(),
        };
        let db = NativeDbClient {
            client: (*self.redis_client).clone(),
        };

        let start = std::time::Instant::now();
        let stressor_results = core::run_workload(&config, &net, &db).await;
        let total_ms = start.elapsed().as_millis() as i64;

        println!(
            "Workload completed in {} ms with results: {:?}",
            total_ms, stressor_results
        );

        Ok(Response::new(WorkloadResponse {
            stressor_results,
            total_ms,
        }))
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn normalize_redis_url(addr: &str) -> String {
    if addr.starts_with("redis://") || addr.starts_with("rediss://") {
        addr.to_string()
    } else {
        format!("redis://{}/", addr)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "50055".to_string());
    let catalog_addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
        .unwrap_or_else(|_| "localhost:3550".to_string());
    let redis_addr =
        std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string());

    let addr = format!("0.0.0.0:{}", port)
        .parse()
        .context("invalid listen address")?;

    let redis_client = Arc::new(
        redis::Client::open(normalize_redis_url(&redis_addr))
            .context("failed to create Redis client")?,
    );

    let service = SyntheticServiceImpl {
        catalog_addr: catalog_addr.clone(),
        redis_client,
    };

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<SyntheticServiceServer<SyntheticServiceImpl>>()
        .await;

    println!(
        "SyntheticService listening on {} (catalog={}, redis={})",
        addr, catalog_addr, redis_addr
    );

    Server::builder()
        .add_service(health_service)
        .add_service(SyntheticServiceServer::new(service))
        .serve(addr)
        .await
        .context("gRPC server failed")?;

    Ok(())
}
