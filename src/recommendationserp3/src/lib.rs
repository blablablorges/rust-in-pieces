//! Recommendation as a wasi:http 0.3 handler; the catalog call goes through the host's
//! h2 client (WASIHOST_H2C_AUTHORITIES must list PRODUCT_CATALOG_SERVICE_ADDR).
use http_body_util::{BodyExt, Full};
use tonic::{Request, Response, Status};
use wasip3::http::types::{ErrorCode, Request as WasiRequest, Response as WasiResponse};
use wasip3::http_compat::{http_from_wasi_request, http_into_wasi_response};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}
#[path = "../../recommendationserwasm/src/core.rs"]
mod core;
#[path = "../../syntheticservicep3/src/grpc.rs"]
mod grpc;

use core::CatalogClient;
use hipstershop::recommendation_service_server::{RecommendationService, RecommendationServiceServer};
use hipstershop::{Empty, ListProductsResponse, ListRecommendationsRequest, ListRecommendationsResponse};

struct HostCatalogClient {
    addr: String,
}

#[tonic::async_trait]
impl CatalogClient for HostCatalogClient {
    async fn list_product_ids(&self) -> Result<Vec<String>, String> {
        let resp: ListProductsResponse =
            grpc::unary(&self.addr, "/hipstershop.ProductCatalogService/ListProducts", &Empty {}).await?;
        Ok(resp.products.into_iter().map(|p| p.id).collect())
    }
}

struct Svc {
    catalog: HostCatalogClient,
}

#[tonic::async_trait]
impl RecommendationService for Svc {
    async fn list_recommendations(
        &self,
        request: Request<ListRecommendationsRequest>,
    ) -> Result<Response<ListRecommendationsResponse>, Status> {
        core::list_recommendations(&self.catalog, request.get_ref()).await.map(Response::new)
    }
}

struct Component;

impl wasip3::exports::http::handler::Guest for Component {
    async fn handle(request: WasiRequest) -> Result<WasiResponse, ErrorCode> {
        let req = http_from_wasi_request(request)?;
        let (parts, body) = req.into_parts();
        let bytes = body.collect().await.map_err(|_| ErrorCode::InternalError(None))?.to_bytes();
        let health = parts.uri.path().starts_with("/grpc.health.v1.Health/");
        let req = http::Request::from_parts(parts, Full::new(bytes));
        let resp = if health {
            let (_reporter, mut svc) = tonic_health::server::health_reporter();
            tonic::codegen::Service::call(&mut svc, req).await
        } else {
            let addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
                .unwrap_or_else(|_| "productcatalogservice:3550".to_string());
            let mut svc = RecommendationServiceServer::new(Svc { catalog: HostCatalogClient { addr } });
            tonic::codegen::Service::call(&mut svc, req).await
        }
        .map_err(|_| ErrorCode::InternalError(None))?;
        http_into_wasi_response(resp)
    }
}

wasip3::http::service::export!(Component);
