use prost::Message;
use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

mod core;

use core::{DbClient, NetworkClient, WorkloadConfig};
use hipstershop::synthetic_service_server::{SyntheticService, SyntheticServiceServer};
use hipstershop::{Empty, ListProductsResponse, WorkloadRequest, WorkloadResponse};

// ---------------------------------------------------------------------------
// WASI network adapter — gRPC call to ProductCatalogService via WASI HTTP
// (same blocking pattern as recommendationserwasm)
// ---------------------------------------------------------------------------

struct WasiNetworkClient {
    addr: String,
}

#[tonic::async_trait]
impl NetworkClient for WasiNetworkClient {
    async fn list_products_once(&self) -> Result<usize, String> {
        let resp = wasi_call_list_products(&self.addr)?;
        Ok(resp.products.len())
    }
}

// ---------------------------------------------------------------------------
// WASI DB adapter — RESP over TCP via wasi:sockets
// (same blocking pattern as cartserwasm)
// ---------------------------------------------------------------------------

struct WasiDbClient {
    addr: String,
}

#[tonic::async_trait]
impl DbClient for WasiDbClient {
    async fn roundtrip(&self, key: &str, value: &[u8]) -> Result<(), String> {
        // One socket for both commands instead of dialing (and re-resolving
        // DNS) twice. Connection is dropped/closed at the end of this scope.
        let conn = RedisConn::connect(&self.addr)?;
        let set_resp = conn.command(&[b"SET", key.as_bytes(), value])?;
        if !set_resp.is_empty() && set_resp[0] == b'-' {
            return Err(format!(
                "Redis SET error: {}",
                std::str::from_utf8(&set_resp[1..]).unwrap_or("?").trim()
            ));
        }
        let get_resp = conn.command(&[b"GET", key.as_bytes()])?;
        parse_bulk_string(&get_resp)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// gRPC component — delegates entirely to core
// ---------------------------------------------------------------------------

#[wasi_grpc_server::grpc_component(SyntheticServiceServer)]
struct SyntheticServiceImpl;

#[tonic::async_trait]
impl SyntheticService for SyntheticServiceImpl {
    async fn run_workload(
        &self,
        _request: Request<WorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        let config = WorkloadConfig::from_env();
        
        println!("Received workload request with config: {:?}", config);
        let catalog_addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
            .unwrap_or_else(|_| "localhost:3550".to_string());
        let redis_addr =
            std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".to_string());

        let net = WasiNetworkClient { addr: catalog_addr };
        let db = WasiDbClient { addr: redis_addr };

        let start = std::time::Instant::now();
        let stressor_results = core::run_workload(&config, &net, &db).await;
        let total_ms = start.elapsed().as_millis() as i64;

        println!(
            "Workload completed in {} ms with results: {:?}",
            total_ms, stressor_results
        );

        Ok(Response::new(WorkloadResponse {
            stressor_results,
            total_ms,
        }))
    }
}

// ---------------------------------------------------------------------------
// WASI HTTP outgoing — calls ProductCatalogService.ListProducts
// ---------------------------------------------------------------------------

fn wasi_call_list_products(addr: &str) -> Result<ListProductsResponse, String> {
    let empty = Empty {};
    let mut proto_buf = Vec::new();
    empty
        .encode(&mut proto_buf)
        .map_err(|e| format!("encode error: {}", e))?;

    let mut grpc_frame = Vec::with_capacity(5 + proto_buf.len());
    grpc_frame.push(0u8);
    grpc_frame.extend_from_slice(&(proto_buf.len() as u32).to_be_bytes());
    grpc_frame.extend_from_slice(&proto_buf);

    let headers = wasi::http::types::Fields::new();
    headers
        .append(&"content-type".into(), &b"application/grpc".to_vec())
        .map_err(|e| format!("content-type error: {:?}", e))?;

    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Post)
        .map_err(|()| "set method failed".to_string())?;
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .map_err(|()| "set scheme failed".to_string())?;
    request
        .set_authority(Some(addr))
        .map_err(|()| "set authority failed".to_string())?;
    request
        .set_path_with_query(Some("/hipstershop.ProductCatalogService/ListProducts"))
        .map_err(|()| "set path failed".to_string())?;

    let body = request.body().map_err(|_| "get body failed".to_string())?;
    let stream = body.write().map_err(|_| "get stream failed".to_string())?;

    let future_response = wasi::http::outgoing_handler::handle(request, None)
        .map_err(|e| format!("handle error: {:?}", e))?;

    stream
        .blocking_write_and_flush(&grpc_frame)
        .map_err(|e| format!("write error: {:?}", e))?;
    drop(stream);
    wasi::http::types::OutgoingBody::finish(body, None)
        .map_err(|e| format!("finish body error: {:?}", e))?;

    future_response.subscribe().block();

    let response = future_response
        .get()
        .ok_or("no response")?
        .map_err(|_| "future error")?
        .map_err(|e| format!("response error: {:?}", e))?;

    if response.status() != 200 {
        return Err(format!("HTTP status {}", response.status()));
    }

    let incoming_body = response.consume().map_err(|_| "consume failed")?;
    let input_stream = incoming_body.stream().map_err(|_| "stream failed")?;
    let mut bytes = Vec::new();
    loop {
        match input_stream.blocking_read(65536) {
            Ok(chunk) if chunk.is_empty() => break,
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(e) => return Err(format!("read error: {:?}", e)),
        }
    }
    drop(input_stream);

    if bytes.len() < 5 {
        return Err(format!("response too short: {} bytes", bytes.len()));
    }
    let msg_len =
        u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    if bytes.len() < 5 + msg_len {
        return Err(format!("incomplete response"));
    }
    ListProductsResponse::decode(&bytes[5..5 + msg_len])
        .map_err(|e| format!("decode error: {}", e))
}

// ---------------------------------------------------------------------------
// WASI TCP/RESP — minimal Redis client (SET + GET) with timeouts
// ---------------------------------------------------------------------------

/// Per-operation timeout (ns) for redis connect / read.
///
/// Without this, a connect to an unreachable redis blocks for the full TCP SYN
/// timeout (~127 s) while the shim pins this request's entire Wasm instance in
/// memory. A handful of such hung requests OOM the shim. Failing fast turns the
/// hang into a counted error so the request returns and its memory is freed.
const REDIS_TIMEOUT_NS: u64 = 5_000_000_000; // 5 s

/// Block on `p` until ready or `timeout_ns` elapses; `Err` on timeout.
fn poll_with_timeout(p: &wasi::io::poll::Pollable, timeout_ns: u64) -> Result<(), String> {
    let timer = wasi::clocks::monotonic_clock::subscribe_duration(timeout_ns);
    // poll() returns the indices of the ready pollables; index 0 is `p`.
    if wasi::io::poll::poll(&[p, &timer]).contains(&0) {
        Ok(())
    } else {
        Err("timed out".to_string())
    }
}

/// An open redis connection. The socket must outlive its streams, so it is
/// held here; dropping `RedisConn` closes everything.
struct RedisConn {
    _sock: wasi::sockets::tcp::TcpSocket,
    input: wasi::io::streams::InputStream,
    output: wasi::io::streams::OutputStream,
}

impl RedisConn {
    fn connect(addr: &str) -> Result<Self, String> {
        let (host, port) = parse_host_port(addr)?;

        let net = wasi::sockets::instance_network::instance_network();
        let addrs = wasi::sockets::ip_name_lookup::resolve_addresses(&net, &host)
            .map_err(|e| format!("DNS error: {:?}", e))?;
        poll_with_timeout(&addrs.subscribe(), REDIS_TIMEOUT_NS)
            .map_err(|e| format!("DNS {}", e))?;
        let ip = addrs
            .resolve_next_address()
            .map_err(|e| format!("DNS error: {:?}", e))?
            .ok_or_else(|| format!("could not resolve {}", host))?;

        let family = match &ip {
            wasi::sockets::network::IpAddress::Ipv4(_) => {
                wasi::sockets::network::IpAddressFamily::Ipv4
            }
            wasi::sockets::network::IpAddress::Ipv6(_) => {
                wasi::sockets::network::IpAddressFamily::Ipv6
            }
        };
        let sock = wasi::sockets::tcp_create_socket::create_tcp_socket(family)
            .map_err(|e| format!("socket create error: {:?}", e))?;

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

        sock.start_connect(&net, remote)
            .map_err(|e| format!("connect error: {:?}", e))?;
        poll_with_timeout(&sock.subscribe(), REDIS_TIMEOUT_NS)
            .map_err(|e| format!("connect {}", e))?;
        let (input, output) = sock
            .finish_connect()
            .map_err(|e| format!("finish_connect error: {:?}", e))?;

        Ok(RedisConn {
            _sock: sock,
            input,
            output,
        })
    }

    /// Send one RESP command and read its reply (timeout-bounded).
    fn command(&self, args: &[&[u8]]) -> Result<Vec<u8>, String> {
        self.output
            .blocking_write_and_flush(&resp_encode(args))
            .map_err(|e| format!("write error: {:?}", e))?;

        // Subscribe once and reuse the pollable across reads. We must return as
        // soon as a full RESP reply is buffered — redis keeps the connection
        // open after replying, so a `blocking_read` past completion would wait
        // forever for bytes that never come.
        let sub = self.input.subscribe();
        let mut buf = Vec::new();
        loop {
            poll_with_timeout(&sub, REDIS_TIMEOUT_NS).map_err(|e| format!("read {}", e))?;
            match self.input.read(4096) {
                Ok(chunk) if chunk.is_empty() => continue,
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
        return Err(format!(
            "Redis error: {}",
            std::str::from_utf8(&buf[1..]).unwrap_or("?").trim()
        ));
    }
    if buf[0] != b'$' {
        return Ok(None);
    }
    let crlf = buf
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or("malformed RESP")?;
    let len: i64 = std::str::from_utf8(&buf[1..crlf])
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    if len < 0 {
        return Ok(None);
    }
    let start = crlf + 2;
    let end = start + len as usize;
    if buf.len() < end {
        return Err("incomplete bulk string".into());
    }
    Ok(Some(buf[start..end].to_vec()))
}

fn parse_host_port(addr: &str) -> Result<(String, u16), String> {
    if let Some(colon) = addr.rfind(':') {
        let port: u16 = addr[colon + 1..]
            .parse()
            .map_err(|_| format!("invalid port: {}", addr))?;
        Ok((addr[..colon].to_string(), port))
    } else {
        Ok((addr.to_string(), 6379))
    }
}
