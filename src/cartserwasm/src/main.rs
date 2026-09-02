use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

mod core;

// Host-pooled raw-TCP (Redis) import. The shim implements this interface,
// owning the connection pool and RESP reply framing, so the guest no longer
// opens a socket per command. Generated with the same wit-bindgen the `wasi`
// crate uses, so they share one wit-bindgen-rt runtime (no duplicate symbols).
mod pooled_tcp_bindings {
    wit_bindgen::generate!({
        inline: r#"
package hybrid:microservices;

interface pooled-tcp {
  variant tcp-error {
    unknown-upstream(string),
    connect-failed(string),
    timeout,
    protocol(string),
    io(string),
  }
  request: func(upstream: string, payload: list<u8>) -> result<list<u8>, tcp-error>;
}

world pooled-tcp-client {
  import pooled-tcp;
}
"#,
    });
}
use pooled_tcp_bindings::hybrid::microservices::pooled_tcp;

use core::CartStore;
use hipstershop::cart_service_server::{CartService, CartServiceServer};
use hipstershop::{AddItemRequest, Cart, Empty, EmptyCartRequest, GetCartRequest};

// ---------------------------------------------------------------------------
// WASI Redis adapter — satisfies CartStore using raw TCP/RESP over wasi:sockets
// ---------------------------------------------------------------------------

struct WasiRedis;

#[tonic::async_trait]
impl CartStore for WasiRedis {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        redis_get(key)
    }

    async fn save(&self, key: &str, data: Vec<u8>) -> Result<(), String> {
        redis_set(key, &data)
    }
}

// ---------------------------------------------------------------------------
// gRPC component — delegates entirely to core logic
// ---------------------------------------------------------------------------

#[wasi_grpc_server::grpc_component(CartServiceServer)]
struct CartServiceImpl;

#[tonic::async_trait]
impl CartService for CartServiceImpl {
    async fn add_item(
        &self,
        request: Request<AddItemRequest>,
    ) -> Result<Response<Empty>, Status> {
        core::add_item(&WasiRedis, request.into_inner())
            .await
            .map(Response::new)
    }

    async fn get_cart(
        &self,
        request: Request<GetCartRequest>,
    ) -> Result<Response<Cart>, Status> {
        core::get_cart(&WasiRedis, request.into_inner().user_id)
            .await
            .map(Response::new)
    }

    async fn empty_cart(
        &self,
        request: Request<EmptyCartRequest>,
    ) -> Result<Response<Empty>, Status> {
        core::empty_cart(&WasiRedis, request.into_inner().user_id)
            .await
            .map(Response::new)
    }
}

// ---------------------------------------------------------------------------
// Minimal RESP client over wasi:sockets/tcp (WASI-specific, stays here)
// ---------------------------------------------------------------------------

fn redis_addr() -> String {
    std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string())
}

fn resp_encode(args: &[&[u8]]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(b'*');
    buf.extend_from_slice(args.len().to_string().as_bytes());
    buf.extend_from_slice(b"\r\n");
    for arg in args {
        buf.push(b'$');
        buf.extend_from_slice(arg.len().to_string().as_bytes());
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(arg);
        buf.extend_from_slice(b"\r\n");
    }
    buf
}

fn parse_bulk_string(buf: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if buf.is_empty() {
        return Err("empty response".into());
    }
    if buf[0] == b'-' {
        let msg = std::str::from_utf8(&buf[1..]).unwrap_or("unknown error").trim();
        return Err(format!("Redis error: {}", msg));
    }
    if buf[0] != b'$' {
        return Ok(None);
    }
    let crlf = buf
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or("malformed RESP")?;
    let len_str =
        std::str::from_utf8(&buf[1..crlf]).map_err(|e| format!("utf8: {}", e))?;
    let len: i64 = len_str.parse().map_err(|e| format!("parse len: {}", e))?;
    if len < 0 {
        return Ok(None);
    }
    let data_start = crlf + 2;
    let data_end = data_start + len as usize;
    if buf.len() < data_end {
        return Err("incomplete bulk string data".into());
    }
    Ok(Some(buf[data_start..data_end].to_vec()))
}

fn redis_command(args: &[&[u8]]) -> Result<Vec<u8>, String> {
    // The shim's host-pooled raw-TCP middleware owns the connection and the RESP
    // reply framing and returns one complete reply, so we just hand it the
    // encoded request. Connection reuse / keepalive / liveness live host-side.
    let addr = redis_addr();
    let payload = resp_encode(args);
    pooled_tcp::request(&addr, &payload).map_err(|e| format!("pooled-tcp request failed: {e:?}"))
}

fn redis_get(key: &str) -> Result<Option<Vec<u8>>, String> {
    let resp = redis_command(&[b"GET", key.as_bytes()])?;
    parse_bulk_string(&resp)
}

fn redis_set(key: &str, value: &[u8]) -> Result<(), String> {
    let resp = redis_command(&[b"SET", key.as_bytes(), value])?;
    if !resp.is_empty() && resp[0] == b'-' {
        let msg = std::str::from_utf8(&resp[1..]).unwrap_or("unknown").trim();
        return Err(format!("Redis SET error: {}", msg));
    }
    Ok(())
}
