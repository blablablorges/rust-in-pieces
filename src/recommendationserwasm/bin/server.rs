use anyhow::{Context, Result};
use tonic::{transport::Server, Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

#[path = "../src/core.rs"]
mod core;

use core::CatalogClient;
use hipstershop::product_catalog_service_client::ProductCatalogServiceClient;
use hipstershop::recommendation_service_server::{RecommendationService, RecommendationServiceServer};
use hipstershop::{Empty, ListRecommendationsRequest, ListRecommendationsResponse};

// ---------------------------------------------------------------------------
// Native catalog adapter — uses a tonic gRPC client
// build_transport(false) means no generated connect() shortcut, so we build
// the channel via tonic::transport::Endpoint directly.
// ---------------------------------------------------------------------------

struct TonicCatalogClient {
    addr: String,
}

#[tonic::async_trait]
impl CatalogClient for TonicCatalogClient {
    async fn list_product_ids(&self) -> Result<Vec<String>, String> {
        let endpoint =
            tonic::transport::Endpoint::from_shared(format!("http://{}", self.addr))
                .map_err(|e| e.to_string())?;
        let channel = endpoint.connect().await.map_err(|e| e.to_string())?;
        let mut client = ProductCatalogServiceClient::new(channel);
        let resp = client
            .list_products(Empty {})
            .await
            .map_err(|e| e.to_string())?;
        Ok(resp.into_inner().products.into_iter().map(|p| p.id).collect())
    }
}

// ---------------------------------------------------------------------------
// gRPC service — delegates entirely to core logic
// ---------------------------------------------------------------------------

struct RecommendationServiceImpl;

#[tonic::async_trait]
impl RecommendationService for RecommendationServiceImpl {
    async fn list_recommendations(
        &self,
        request: Request<ListRecommendationsRequest>,
    ) -> Result<Response<ListRecommendationsResponse>, Status> {
        let addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
            .unwrap_or_else(|_| "localhost:3550".to_string());
        let client = TonicCatalogClient { addr };
        core::list_recommendations(&client, request.get_ref())
            .await
            .map(Response::new)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{}", port)
        .parse()
        .context("invalid listen address")?;

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<RecommendationServiceServer<RecommendationServiceImpl>>()
        .await;

    println!("RecommendationService listening on {}", addr);

    Server::builder()
        .add_service(health_service)
        .add_service(RecommendationServiceServer::new(RecommendationServiceImpl))
        .serve(addr)
        .await
        .context("gRPC server failed")?;

    Ok(())
}
