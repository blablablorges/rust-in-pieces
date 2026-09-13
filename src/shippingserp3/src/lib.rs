//! Shipping as a wasi:http 0.3 handler: gRPC (ShippingService) and health on one port.
//! The service logic is the P2 crate's core.rs, included by path.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tonic::{Request, Response, Status};
use wasip3::http::types::{ErrorCode, Request as WasiRequest, Response as WasiResponse};
use wasip3::http_compat::{http_from_wasi_request, http_into_wasi_response};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}
#[path = "../../shippingserwasm/src/core.rs"]
mod core;

use hipstershop::shipping_service_server::{ShippingService, ShippingServiceServer};
use hipstershop::{GetQuoteRequest, GetQuoteResponse, Money, ShipOrderRequest, ShipOrderResponse};

const NANOS_PER_CENT: i32 = 10_000_000;

struct Svc;

#[tonic::async_trait]
impl ShippingService for Svc {
    async fn get_quote(
        &self,
        request: Request<GetQuoteRequest>,
    ) -> Result<Response<GetQuoteResponse>, Status> {
        let item_count: u32 = request.get_ref().items.iter().map(|i| i.quantity as u32).sum();
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
        let base_address = format!("{}, {}, {}", address.street_address, address.city, address.state);
        Ok(Response::new(ShipOrderResponse { tracking_id: core::create_tracking_id(&base_address) }))
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
            let mut svc = ShippingServiceServer::new(Svc);
            tonic::codegen::Service::call(&mut svc, req).await
        }
        .map_err(|_| ErrorCode::InternalError(None))?;
        http_into_wasi_response(resp)
    }
}

wasip3::http::service::export!(Component);
