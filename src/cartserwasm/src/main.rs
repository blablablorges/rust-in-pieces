use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

mod core;

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

fn read_resp_response(input: &wasi::io::streams::InputStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    loop {
        match input.blocking_read(4096) {
            Ok(chunk) => {
                buf.extend_from_slice(&chunk);
                if resp_is_complete(&buf) {
                    return Ok(buf);
                }
            }
            Err(wasi::io::streams::StreamError::Closed) => return Ok(buf),
            Err(e) => return Err(format!("read error: {:?}", e)),
        }
    }
}

fn resp_is_complete(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return false;
    }
    match buf[0] {
        b'+' | b'-' | b':' => buf.windows(2).any(|w| w == b"\r\n"),
        b'$' => {
            if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                let len_str = std::str::from_utf8(&buf[1..pos]).unwrap_or("");
                if let Ok(len) = len_str.parse::<i64>() {
                    if len < 0 {
                        return true;
                    }
                    buf.len() >= pos + 2 + len as usize + 2
                } else {
                    false
                }
            } else {
                false
            }
        }
        _ => buf.windows(2).any(|w| w == b"\r\n"),
    }
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
    let addr_str = redis_addr();
    let (host, port) = parse_host_port(&addr_str)?;

    let net = wasi::sockets::instance_network::instance_network();
    let addrs = wasi::sockets::ip_name_lookup::resolve_addresses(&net, &host)
        .map_err(|e| format!("DNS resolve error: {:?}", e))?;
    addrs.subscribe().block();
    let ip = addrs
        .resolve_next_address()
        .map_err(|e| format!("DNS error: {:?}", e))?
        .ok_or_else(|| format!("could not resolve host: {}", host))?;

    let sock = match &ip {
        wasi::sockets::network::IpAddress::Ipv4(_) => {
            wasi::sockets::tcp_create_socket::create_tcp_socket(
                wasi::sockets::network::IpAddressFamily::Ipv4,
            )
        }
        wasi::sockets::network::IpAddress::Ipv6(_) => {
            wasi::sockets::tcp_create_socket::create_tcp_socket(
                wasi::sockets::network::IpAddressFamily::Ipv6,
            )
        }
    }
    .map_err(|e| format!("create socket error: {:?}", e))?;

    let remote = match ip {
        wasi::sockets::network::IpAddress::Ipv4(a) => {
            wasi::sockets::network::IpSocketAddress::Ipv4(
                wasi::sockets::network::Ipv4SocketAddress { address: a, port },
            )
        }
        wasi::sockets::network::IpAddress::Ipv6(a) => {
            wasi::sockets::network::IpSocketAddress::Ipv6(
                wasi::sockets::network::Ipv6SocketAddress {
                    address: a,
                    port,
                    flow_info: 0,
                    scope_id: 0,
                },
            )
        }
    };

    let net = wasi::sockets::instance_network::instance_network();
    sock.start_connect(&net, remote)
        .map_err(|e| format!("start_connect error: {:?}", e))?;
    sock.subscribe().block();
    let (input, output) = sock
        .finish_connect()
        .map_err(|e| format!("finish_connect error: {:?}", e))?;

    output
        .blocking_write_and_flush(&resp_encode(args))
        .map_err(|e| format!("write error: {:?}", e))?;

    read_resp_response(&input)
}

fn parse_host_port(addr: &str) -> Result<(String, u16), String> {
    if let Some(colon) = addr.rfind(':') {
        let port: u16 = addr[colon + 1..]
            .parse()
            .map_err(|_| format!("invalid port in: {}", addr))?;
        Ok((addr[..colon].to_string(), port))
    } else {
        Ok((addr.to_string(), 6379))
    }
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
