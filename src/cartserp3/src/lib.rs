//! Cart as a wasi:http 0.3 handler. The redis connection is one 0.3 socket per instance,
//! held across requests behind an async mutex (the P2 port caches one multiplexed
//! connection the same way).
use std::sync::OnceLock;

use http_body_util::{BodyExt, Full};
use tonic::{Request, Response, Status};
use wasip3::http::types::{ErrorCode, Request as WasiRequest, Response as WasiResponse};
use wasip3::http_compat::{http_from_wasi_request, http_into_wasi_response};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}
#[path = "../../cartserwasm/src/core.rs"]
mod core;
#[path = "../../syntheticservicep3/src/tcp.rs"]
mod tcp;
#[path = "../../syntheticservicep3/src/resp.rs"]
mod resp;

use core::CartStore;
use hipstershop::cart_service_server::{CartService, CartServiceServer};
use hipstershop::{AddItemRequest, Cart, Empty, EmptyCartRequest, GetCartRequest};

struct RedisStore(resp::Client);

#[tonic::async_trait]
impl CartStore for RedisStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.0.get(key).await
    }
    async fn save(&self, key: &str, data: Vec<u8>) -> Result<(), String> {
        self.0.set(key, &data).await
    }
}

static STORE: OnceLock<RedisStore> = OnceLock::new();

fn store() -> Result<&'static RedisStore, String> {
    if let Some(s) = STORE.get() {
        return Ok(s);
    }
    let addr = std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string());
    let s = RedisStore(resp::Client::new(&addr)?);
    Ok(STORE.get_or_init(|| s))
}

struct Svc(&'static RedisStore);

#[tonic::async_trait]
impl CartService for Svc {
    async fn add_item(&self, request: Request<AddItemRequest>) -> Result<Response<Empty>, Status> {
        core::add_item(self.0, request.into_inner()).await.map(Response::new)
    }
    async fn get_cart(&self, request: Request<GetCartRequest>) -> Result<Response<Cart>, Status> {
        core::get_cart(self.0, request.into_inner().user_id).await.map(Response::new)
    }
    async fn empty_cart(&self, request: Request<EmptyCartRequest>) -> Result<Response<Empty>, Status> {
        core::empty_cart(self.0, request.into_inner().user_id).await.map(Response::new)
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
            let store = store().map_err(|_| ErrorCode::InternalError(None))?;
            let mut svc = CartServiceServer::new(Svc(store));
            tonic::codegen::Service::call(&mut svc, req).await
        }
        .map_err(|_| ErrorCode::InternalError(None))?;
        http_into_wasi_response(resp)
    }
}

wasip3::http::service::export!(Component);
