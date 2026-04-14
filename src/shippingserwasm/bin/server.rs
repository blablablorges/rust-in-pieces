use anyhow::{Context, Result};
use tonic::{transport::Server, Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

#[path = "../src/core.rs"]
mod core;

use hipstershop::shipping_service_server::{ShippingService, ShippingServiceServer};
use hipstershop::{GetQuoteRequest, GetQuoteResponse, Money, ShipOrderRequest, ShipOrderResponse};

const NANOS_PER_CENT: i32 = 10_000_000;

struct ShippingServiceImpl;

#[tonic::async_trait]
impl ShippingService for ShippingServiceImpl {
    async fn get_quote(
        &self,
        request: Request<GetQuoteRequest>,
    ) -> Result<Response<GetQuoteResponse>, Status> {
        let item_count: u32 = request
            .get_ref()
            .items
            .iter()
            .map(|i| i.quantity as u32)
            .sum();
        let q = core::compute_quote(item_count);
        Ok(Response::new(GetQuoteResponse {
            cost_usd: Some(Money {
                currency_code: "USD".into(),
                units: q.dollars,
                nanos: q.cents * NANOS_PER_CENT,
            }),
        }))
    }

    async fn ship_order(
        &self,
        request: Request<ShipOrderRequest>,
    ) -> Result<Response<ShipOrderResponse>, Status> {
        let address = request
            .get_ref()
            .address
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("Address is required"))?;
        let base_address = format!(
            "{}, {}, {}",
            address.street_address, address.city, address.state
        );
        Ok(Response::new(ShipOrderResponse {
            tracking_id: core::create_tracking_id(&base_address),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "50051".to_string());
    let addr = format!("0.0.0.0:{}", port)
        .parse()
        .context("invalid listen address")?;

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<ShippingServiceServer<ShippingServiceImpl>>()
        .await;

    println!("ShippingService listening on {}", addr);

    Server::builder()
        .add_service(health_service)
        .add_service(ShippingServiceServer::new(ShippingServiceImpl))
        .serve(addr)
        .await
        .context("gRPC server failed")?;

    Ok(())
}
