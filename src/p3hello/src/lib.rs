//! p3hello: a wasi:http 0.3 handler that serves SyntheticService.RunWorkload over gRPC and
//! calls ProductCatalogService.ListProducts over gRPC through the host's outbound handler.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use prost::Message;
use wasip3::http::types::{ErrorCode, Request, Response};
use wasip3::http_compat::{
    http_from_wasi_request, http_from_wasi_response, http_into_wasi_request, http_into_wasi_response,
};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}
use hipstershop::synthetic_service_server::{SyntheticService, SyntheticServiceServer};
use hipstershop::{ListProductsResponse, WorkloadRequest, WorkloadResponse};

async fn list_products() -> Result<usize, String> {
    let addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR").unwrap_or_else(|_| "localhost:3550".into());
    // Empty{} encodes to zero bytes: a bare 5-byte gRPC frame prefix.
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{addr}/hipstershop.ProductCatalogService/ListProducts"))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Full::new(Bytes::from_static(&[0u8; 5])))
        .map_err(|e| e.to_string())?;
    let wreq = http_into_wasi_request(req).map_err(|e| format!("request: {e:?}"))?;
    let wresp = wasip3::http::client::send(wreq).await.map_err(|e| format!("handle: {e:?}"))?;
    let resp = http_from_wasi_response(wresp).map_err(|e| format!("response: {e:?}"))?;
    let (parts, body) = resp.into_parts();
    let collected = body.collect().await.map_err(|e| format!("body: {e:?}"))?;
    let status = collected
        .trailers()
        .and_then(|t| t.get("grpc-status"))
        .or_else(|| parts.headers.get("grpc-status"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if status.as_deref() != Some("0") {
        return Err(format!("grpc-status {status:?}, http {}", parts.status));
    }
    let data = collected.to_bytes();
    if data.len() < 5 {
        return Err(format!("short gRPC frame: {} bytes", data.len()));
    }
    let msg = ListProductsResponse::decode(&data[5..]).map_err(|e| e.to_string())?;
    Ok(msg.products.len())
}

struct Svc;

#[tonic::async_trait]
impl SyntheticService for Svc {
    async fn run_workload(
        &self,
        _request: tonic::Request<WorkloadRequest>,
    ) -> Result<tonic::Response<WorkloadResponse>, tonic::Status> {
        let start = std::time::Instant::now();
        let n = list_products().await.map_err(tonic::Status::unavailable)?;
        Ok(tonic::Response::new(WorkloadResponse {
            stressor_results: std::collections::HashMap::from([("network".to_string(), format!("{n} products"))]),
            total_ms: start.elapsed().as_millis() as i64,
        }))
    }
}

struct Component;

impl wasip3::exports::http::handler::Guest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let req = http_from_wasi_request(request)?;
        let (parts, body) = req.into_parts();
        let bytes = body.collect().await.map_err(|_| ErrorCode::InternalError(None))?.to_bytes();
        let req = http::Request::from_parts(parts, Full::new(bytes));
        let mut svc = SyntheticServiceServer::new(Svc);
        let resp = tonic::codegen::Service::call(&mut svc, req)
            .await
            .map_err(|_| ErrorCode::InternalError(None))?;
        http_into_wasi_response(resp)
    }
}

wasip3::http::service::export!(Component);
