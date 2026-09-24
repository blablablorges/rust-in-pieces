//! Synthetic service as a wasi:http 0.3 handler: gRPC (SyntheticService.RunWorkload) and
//! POST /workload JSON on the same port; outbound gRPC to productcatalog through the host's
//! client import; redis over wasi:sockets 0.3 with a hand-rolled RESP SET/GET.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use std::collections::HashMap;
use wasip3::http::types::{ErrorCode, Request, Response};
use wasip3::http_compat::{http_from_wasi_request, http_into_wasi_response};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}
mod core;
mod tcp;
mod resp;
mod grpc;

use core::{DbClient, NetworkClient, WorkloadConfig};
use hipstershop::synthetic_service_server::{SyntheticService, SyntheticServiceServer};
use hipstershop::{Empty, ListProductsResponse, WorkloadRequest, WorkloadResponse};

// ---------------------------------------------------------------- outbound gRPC ---

struct WasiNetworkClient {
    addr: String,
}

#[tonic::async_trait]
impl NetworkClient for WasiNetworkClient {
    async fn list_products_once(&self) -> Result<usize, String> {
        let resp: ListProductsResponse = grpc::unary(
            &self.addr,
            "/hipstershop.ProductCatalogService/ListProducts",
            &hipstershop::Empty {},
        )
        .await?;
        Ok(resp.products.len())
    }
}

// ------------------------------------------------------------ redis over sockets ---

struct WasiDbClient {
    addr: String,
}

#[tonic::async_trait]
impl DbClient for WasiDbClient {
    async fn roundtrip(&self, key: &str, value: &[u8]) -> Result<(), String> {
        // One connection per roundtrip, as before: the db mode's known artefact, unchanged here.
        let r = async {
            let c = resp::Client::new(&self.addr)?;
            c.set(key, value).await?;
            c.get(key).await.map(|_| ())
        }
        .await;
        if let Err(e) = &r {
            eprintln!("redis {}: {e}", self.addr);
        }
        r
    }
}

// ------------------------------------------------------------------- service ---

struct Svc {
    net: WasiNetworkClient,
    db: WasiDbClient,
}

impl Svc {
    fn from_env() -> Self {
        Svc {
            net: WasiNetworkClient {
                addr: std::env::var("PRODUCT_CATALOG_SERVICE_ADDR").unwrap_or_else(|_| "localhost:3550".into()),
            },
            db: WasiDbClient { addr: std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".into()) },
        }
    }

    async fn workload(&self) -> (HashMap<String, String>, i64) {
        let config = WorkloadConfig::from_env();
        let start = std::time::Instant::now();
        let results = core::run_workload(&config, &self.net, &self.db).await;
        (results, start.elapsed().as_millis() as i64)
    }
}

#[tonic::async_trait]
impl SyntheticService for Svc {
    async fn run_workload(
        &self,
        _request: tonic::Request<WorkloadRequest>,
    ) -> Result<tonic::Response<WorkloadResponse>, tonic::Status> {
        let (stressor_results, total_ms) = self.workload().await;
        Ok(tonic::Response::new(WorkloadResponse { stressor_results, total_ms }))
    }
}

fn json(results: &HashMap<String, String>, total_ms: i64) -> String {
    let mut s = String::from("{\"stressor_results\":{");
    for (i, (k, v)) in results.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{}\":\"{}\"", json_escape(k), json_escape(v)));
    }
    s.push_str(&format!("}},\"total_ms\":{total_ms}}}"));
    s
}

fn json_escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_keeps_utf8_and_escapes_what_json_requires() {
        assert_eq!(super::json_escape("found 17984 primes ≤ 200000"), "found 17984 primes ≤ 200000");
        assert_eq!(super::json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\u000a");
    }
}

struct Component;

impl wasip3::exports::http::handler::Guest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let req = http_from_wasi_request(request)?;
        let (parts, body) = req.into_parts();
        let bytes = body.collect().await.map_err(|_| ErrorCode::InternalError(None))?.to_bytes();
        let grpc = parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("application/grpc"))
            .unwrap_or(false);
        if grpc {
            let health = parts.uri.path().starts_with("/grpc.health.v1.Health/");
            let req = http::Request::from_parts(parts, Full::new(bytes));
            let resp = if health {
                let (_reporter, mut svc) = tonic_health::server::health_reporter();
                tonic::codegen::Service::call(&mut svc, req).await
            } else {
                let mut svc = SyntheticServiceServer::new(Svc::from_env());
                tonic::codegen::Service::call(&mut svc, req).await
            }
            .map_err(|_| ErrorCode::InternalError(None))?;
            return http_into_wasi_response(resp);
        }
        let (status, body) = if parts.method == http::Method::POST && parts.uri.path() == "/workload" {
            let (results, total_ms) = Svc::from_env().workload().await;
            (200, json(&results, total_ms))
        } else {
            (404, "{\"error\":\"not found\"}".to_string())
        };
        let resp = http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .header("content-length", body.len())
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| ErrorCode::InternalError(None))?;
        http_into_wasi_response(resp)
    }
}

wasip3::http::service::export!(Component);
