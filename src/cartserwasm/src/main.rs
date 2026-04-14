use prost::Message;
use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

use hipstershop::cart_service_server::{CartService, CartServiceServer};
use hipstershop::{AddItemRequest, Cart, CartItem, Empty, EmptyCartRequest, GetCartRequest};

#[wasi_grpc_server::grpc_component(CartServiceServer)]
struct CartServiceImpl;

// ---------------------------------------------------------------------------
// Minimal RESP client over wasi:sockets/tcp
// ---------------------------------------------------------------------------

fn redis_addr() -> String {
    std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string())
}

fn cart_key(user_id: &str) -> String {
    format!("cart:{}", user_id)
}

/// Encode a RESP array command.  e.g. ["GET", "cart:u1"] →
/// `*2\r\n$3\r\nGET\r\n$7\r\ncart:u1\r\n`
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

/// Read from a WASI input stream until we have at least one complete RESP
/// response.  Returns the raw bytes of the response.
fn read_resp_response(input: &wasi::io::streams::InputStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    loop {
        match input.blocking_read(4096) {
            Ok(chunk) => {
                buf.extend_from_slice(&chunk);
                // A complete RESP response ends with \r\n and starts with a
                // type prefix (+, -, :, $, *).  For our needs we can check
                // that we have a full bulk-string or simple response.
                if resp_is_complete(&buf) {
                    return Ok(buf);
                }
            }
            Err(wasi::io::streams::StreamError::Closed) => return Ok(buf),
            Err(e) => return Err(format!("read error: {:?}", e)),
        }
    }
}

/// Very minimal completeness check for RESP responses we care about.
fn resp_is_complete(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return false;
    }
    match buf[0] {
        b'+' | b'-' | b':' => buf.windows(2).any(|w| w == b"\r\n"),
        b'$' => {
            // Bulk string: $<len>\r\n<data>\r\n  or  $-1\r\n (nil)
            if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                let len_str = std::str::from_utf8(&buf[1..pos]).unwrap_or("");
                if let Ok(len) = len_str.parse::<i64>() {
                    if len < 0 {
                        return true; // $-1\r\n
                    }
                    let expected = pos + 2 + len as usize + 2;
                    buf.len() >= expected
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

/// Parse a RESP bulk string response into Option<Vec<u8>>.
/// Returns None for nil ($-1).
fn parse_bulk_string(buf: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if buf.is_empty() {
        return Err("empty response".into());
    }
    if buf[0] == b'-' {
        // Error response
        let msg = std::str::from_utf8(&buf[1..]).unwrap_or("unknown error").trim();
        return Err(format!("Redis error: {}", msg));
    }
    if buf[0] != b'$' {
        // Simple string "+OK\r\n" or integer, just return None
        return Ok(None);
    }
    let crlf = buf.windows(2).position(|w| w == b"\r\n")
        .ok_or("malformed RESP")?;
    let len_str = std::str::from_utf8(&buf[1..crlf]).map_err(|e| format!("utf8: {}", e))?;
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

/// Execute a single Redis command and return the raw response.
fn redis_command(args: &[&[u8]]) -> Result<Vec<u8>, String> {
    let addr_str = redis_addr();
    let (host, port) = parse_host_port(&addr_str)?;

    // Resolve hostname
    let net = wasi::sockets::instance_network::instance_network();
    let addrs = wasi::sockets::ip_name_lookup::resolve_addresses(
        &net,
        &host,
    ).map_err(|e| format!("DNS resolve error: {:?}", e))?;

    let pollable = addrs.subscribe();
    pollable.block();

    let ip = addrs.resolve_next_address()
        .map_err(|e| format!("DNS error: {:?}", e))?
        .ok_or_else(|| format!("could not resolve host: {}", host))?;

    // Create TCP socket
    let sock = match &ip {
        wasi::sockets::network::IpAddress::Ipv4(_) =>
            wasi::sockets::tcp_create_socket::create_tcp_socket(
                wasi::sockets::network::IpAddressFamily::Ipv4,
            ),
        wasi::sockets::network::IpAddress::Ipv6(_) =>
            wasi::sockets::tcp_create_socket::create_tcp_socket(
                wasi::sockets::network::IpAddressFamily::Ipv6,
            ),
    }.map_err(|e| format!("create socket error: {:?}", e))?;

    let remote = match ip {
        wasi::sockets::network::IpAddress::Ipv4(a) =>
            wasi::sockets::network::IpSocketAddress::Ipv4(
                wasi::sockets::network::Ipv4SocketAddress { address: a, port },
            ),
        wasi::sockets::network::IpAddress::Ipv6(a) =>
            wasi::sockets::network::IpSocketAddress::Ipv6(
                wasi::sockets::network::Ipv6SocketAddress {
                    address: a, port, flow_info: 0, scope_id: 0,
                },
            ),
    };

    let net = wasi::sockets::instance_network::instance_network();
    sock.start_connect(&net, remote)
        .map_err(|e| format!("start_connect error: {:?}", e))?;

    let poll = sock.subscribe();
    poll.block();

    let (input, output) = sock.finish_connect()
        .map_err(|e| format!("finish_connect error: {:?}", e))?;

    // Send command
    let cmd = resp_encode(args);
    output.blocking_write_and_flush(&cmd)
        .map_err(|e| format!("write error: {:?}", e))?;

    // Read response
    let resp = read_resp_response(&input)?;

    Ok(resp)
}

fn parse_host_port(addr: &str) -> Result<(String, u16), String> {
    // Handle "host:port" format
    if let Some(colon) = addr.rfind(':') {
        let host = &addr[..colon];
        let port: u16 = addr[colon + 1..].parse()
            .map_err(|_| format!("invalid port in: {}", addr))?;
        Ok((host.to_string(), port))
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
    // Check for error response
    if !resp.is_empty() && resp[0] == b'-' {
        let msg = std::str::from_utf8(&resp[1..]).unwrap_or("unknown").trim();
        return Err(format!("Redis SET error: {}", msg));
    }
    Ok(())
}

fn get_cart_from_redis(user_id: &str) -> Result<Cart, Status> {
    let key = cart_key(user_id);
    match redis_get(&key) {
        Ok(Some(bytes)) => Cart::decode(bytes.as_slice())
            .map_err(|e| Status::internal(format!("failed to decode cart: {}", e))),
        Ok(None) => Ok(Cart {
            user_id: user_id.to_string(),
            items: Vec::new(),
        }),
        Err(e) => Err(Status::failed_precondition(format!(
            "can't access cart storage: {}",
            e
        ))),
    }
}

fn set_cart_to_redis(user_id: &str, cart: &Cart) -> Result<(), Status> {
    let key = cart_key(user_id);
    let mut bytes = Vec::new();
    cart.encode(&mut bytes)
        .map_err(|e| Status::internal(format!("failed to encode cart: {}", e)))?;
    redis_set(&key, &bytes)
        .map_err(|e| Status::failed_precondition(format!("can't access cart storage: {}", e)))
}

#[tonic::async_trait]
impl CartService for CartServiceImpl {
    async fn add_item(&self, request: Request<AddItemRequest>) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let user_id = req.user_id;
        let item = req
            .item
            .ok_or_else(|| Status::invalid_argument("item is required"))?;

        if user_id.is_empty() {
            return Err(Status::invalid_argument("user_id is required"));
        }
        if item.product_id.is_empty() {
            return Err(Status::invalid_argument("product_id is required"));
        }
        if item.quantity <= 0 {
            return Err(Status::invalid_argument("quantity must be positive"));
        }

        let mut cart = get_cart_from_redis(&user_id)?;

        if let Some(existing) = cart.items.iter_mut().find(|i| i.product_id == item.product_id) {
            existing.quantity += item.quantity;
        } else {
            cart.items.push(CartItem {
                product_id: item.product_id,
                quantity: item.quantity,
            });
        }

        set_cart_to_redis(&user_id, &cart)?;

        Ok(Response::new(Empty {}))
    }

    async fn get_cart(&self, request: Request<GetCartRequest>) -> Result<Response<Cart>, Status> {
        let user_id = request.into_inner().user_id;
        if user_id.is_empty() {
            return Err(Status::invalid_argument("user_id is required"));
        }

        let cart = get_cart_from_redis(&user_id)?;

        Ok(Response::new(cart))
    }

    async fn empty_cart(&self, request: Request<EmptyCartRequest>) -> Result<Response<Empty>, Status> {
        let user_id = request.into_inner().user_id;
        if user_id.is_empty() {
            return Err(Status::invalid_argument("user_id is required"));
        }

        let empty_cart = Cart {
            user_id: user_id.clone(),
            items: Vec::new(),
        };
        set_cart_to_redis(&user_id, &empty_cart)?;

        Ok(Response::new(Empty {}))
    }
}
