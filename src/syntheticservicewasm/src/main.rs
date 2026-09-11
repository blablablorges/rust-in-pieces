use anyhow::{Context, Result};
use redis::AsyncCommands;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

mod core;

use core::{DbClient, NetworkClient, WorkloadConfig};
use hipstershop::product_catalog_service_client::ProductCatalogServiceClient;
use hipstershop::synthetic_service_server::{SyntheticService, SyntheticServiceServer};
use hipstershop::{Empty, WorkloadRequest, WorkloadResponse};

// ---------------------------------------------------------------------------
// Network adapter — one lazy tonic channel for the process lifetime, as the
// Python/Go originals hold one grpc channel; the previous native code dialed a
// fresh connection per call, which the wasm side would have paid for too.
// See src/lib/tonic/HYRBID-PATCH.md for what it took to build `channel` on
// wasm32-wasip2.
// ---------------------------------------------------------------------------

struct TonicNetworkClient {
    channel: Channel,
}

#[tonic::async_trait]
impl NetworkClient for TonicNetworkClient {
    async fn list_products_once(&self) -> Result<usize, String> {
        let resp = ProductCatalogServiceClient::new(self.channel.clone())
            .list_products(Empty {})
            .await
            .map_err(|e| e.to_string())?;
        Ok(resp.into_inner().products.len())
    }
}

// ---------------------------------------------------------------------------
// DB adapter — the redis crate, same source on native and wasip2
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

struct SyntheticServiceImpl {
    net: TonicNetworkClient,
    redis_client: Arc<redis::Client>,
}

#[tonic::async_trait]
impl SyntheticService for SyntheticServiceImpl {
    async fn run_workload(
        &self,
        _request: Request<WorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        let config = WorkloadConfig::from_env();
        let db = NativeDbClient {
            client: (*self.redis_client).clone(),
        };

        let start = std::time::Instant::now();
        let stressor_results = core::run_workload(&config, &self.net, &db).await;
        let total_ms = start.elapsed().as_millis() as i64;

        Ok(Response::new(WorkloadResponse {
            stressor_results,
            total_ms,
        }))
    }
}

#[derive(serde::Serialize)]
struct WorkloadJson {
    stressor_results: std::collections::HashMap<String, String>,
    total_ms: i64,
}

async fn rest_workload(
    axum::extract::State(state): axum::extract::State<Arc<SyntheticServiceImpl>>,
) -> axum::Json<WorkloadJson> {
    let config = WorkloadConfig::from_env();
    let db = NativeDbClient { client: (*state.redis_client).clone() };
    let start = std::time::Instant::now();
    let stressor_results = core::run_workload(&config, &state.net, &db).await;
    axum::Json(WorkloadJson { stressor_results, total_ms: start.elapsed().as_millis() as i64 })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

// The redis client hands the host straight to tokio's resolver, which runs the
// lookup on a blocking thread wasip2 cannot spawn (it panics on the first
// request). Resolve once here with std and give redis an IP literal, which
// tokio short-circuits before that spawn — same reason as the catalog address.
fn redis_url(addr: &str) -> Result<String> {
    let hostport = addr.strip_prefix("redis://").unwrap_or(addr).trim_end_matches('/');
    let sock = std::net::ToSocketAddrs::to_socket_addrs(hostport)
        .ok()
        .and_then(|mut it| it.next())
        .with_context(|| format!("cannot resolve {}", addr))?;
    Ok(format!("redis://{}/", sock))
}

// WP-J1/H3: one source for both targets. current_thread on both — wasip2 has no
// threads, and the schedule has to be identical for the comparison to be fair.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "50055".to_string())
        .parse()
        .context("invalid PORT")?;
    let catalog_addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
        .unwrap_or_else(|_| "localhost:3550".to_string());
    let redis_addr =
        std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string());
    let addr = format!("0.0.0.0:{}", port);

    // Resolve here, not in the connector: hyper-util's GaiResolver runs the
    // lookup on tokio's blocking pool, and wasip2 cannot spawn a thread. An IP
    // literal makes HttpConnector skip the resolver entirely. A ClusterIP is
    // stable for the life of the Service; if the lookup fails the pod restarts.
    let catalog_sock = std::net::ToSocketAddrs::to_socket_addrs(&catalog_addr)
        .ok()
        .and_then(|mut it| it.next())
        .with_context(|| format!("cannot resolve {}", catalog_addr))?;

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .context("bind failed")?;

    let redis_client = Arc::new(
        redis::Client::open(redis_url(&redis_addr)?)
            .context("failed to create Redis client")?,
    );

    let service = SyntheticServiceImpl {
        net: TonicNetworkClient {
            channel: Endpoint::from_shared(format!("http://{}", catalog_sock))
                .context("invalid PRODUCT_CATALOG_SERVICE_ADDR")?
                .connect_lazy(),
        },
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

    let service = Arc::new(service);
    let rest = axum::Router::new()
        .route("/workload", axum::routing::post(rest_workload))
        .with_state(service.clone());
    let routes = tonic::service::Routes::from(rest)
        .add_service(health_service)
        .add_service(SyntheticServiceServer::from_arc(service));
    Server::builder()
        .accept_http1(true)
        .add_routes(routes)
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await
        .context("gRPC server failed")?;

    Ok(())
}
