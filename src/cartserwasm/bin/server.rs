use anyhow::{Context, Result};
use redis::AsyncCommands;
use std::sync::Arc;
use tonic::{transport::Server, Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

#[path = "../src/core.rs"]
mod core;

use core::CartStore;
use hipstershop::cart_service_server::{CartService, CartServiceServer};
use hipstershop::{AddItemRequest, Cart, Empty, EmptyCartRequest, GetCartRequest};

// ---------------------------------------------------------------------------
// Native Redis adapter — satisfies CartStore using the redis crate
// ---------------------------------------------------------------------------

struct RedisStore {
    client: redis::Client,
}

#[tonic::async_trait]
impl CartStore for RedisStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| e.to_string())?;
        conn.get(key).await.map_err(|e| e.to_string())
    }

    async fn save(&self, key: &str, data: Vec<u8>) -> Result<(), String> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| e.to_string())?;
        conn.set::<_, _, ()>(key, data)
            .await
            .map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// gRPC service — delegates entirely to core logic
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CartServiceImpl {
    store: Arc<RedisStore>,
}

#[tonic::async_trait]
impl CartService for CartServiceImpl {
    async fn add_item(
        &self,
        request: Request<AddItemRequest>,
    ) -> Result<Response<Empty>, Status> {
        core::add_item(self.store.as_ref(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn get_cart(
        &self,
        request: Request<GetCartRequest>,
    ) -> Result<Response<Cart>, Status> {
        core::get_cart(self.store.as_ref(), request.into_inner().user_id)
            .await
            .map(Response::new)
    }

    async fn empty_cart(
        &self,
        request: Request<EmptyCartRequest>,
    ) -> Result<Response<Empty>, Status> {
        core::empty_cart(self.store.as_ref(), request.into_inner().user_id)
            .await
            .map(Response::new)
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
    let port = std::env::var("PORT").unwrap_or_else(|_| "7070".to_string());
    let redis_addr =
        std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string());

    let addr = format!("0.0.0.0:{}", port)
        .parse()
        .context("invalid listen address")?;

    let client = redis::Client::open(normalize_redis_url(&redis_addr))
        .context("failed to create Redis client")?;
    let service = CartServiceImpl {
        store: Arc::new(RedisStore { client }),
    };

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<CartServiceServer<CartServiceImpl>>()
        .await;

    println!("CartService listening on {} (redis={})", addr, redis_addr);

    Server::builder()
        .add_service(health_service)
        .add_service(CartServiceServer::new(service))
        .serve(addr)
        .await
        .context("gRPC server failed")?;

    Ok(())
}
