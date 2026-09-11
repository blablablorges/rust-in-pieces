//! Synthetic service as a wasi:http 0.3 handler: gRPC (SyntheticService.RunWorkload) and
//! POST /workload JSON on the same port; outbound gRPC to productcatalog through the host's
//! client import; redis over wasi:sockets 0.3 with a hand-rolled RESP SET/GET.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use prost::Message;
use std::collections::HashMap;
use wasip3::http::types::{ErrorCode, Request, Response};
use wasip3::http_compat::{
    http_from_wasi_request, http_from_wasi_response, http_into_wasi_request, http_into_wasi_response,
};
use wasip3::sockets::ip_name_lookup::resolve_addresses;
use wasip3::sockets::types::{
    IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress, TcpSocket,
};
use wasip3::wit_stream;

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}
mod core;

use core::{DbClient, NetworkClient, WorkloadConfig};
use hipstershop::synthetic_service_server::{SyntheticService, SyntheticServiceServer};
use hipstershop::{ListProductsResponse, WorkloadRequest, WorkloadResponse};

// ---------------------------------------------------------------- outbound gRPC ---

struct WasiNetworkClient {
    addr: String,
}

#[tonic::async_trait]
impl NetworkClient for WasiNetworkClient {
    async fn list_products_once(&self) -> Result<usize, String> {
        let req = http::Request::builder()
            .method("POST")
            .uri(format!("http://{}/hipstershop.ProductCatalogService/ListProducts", self.addr))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .body(Full::new(Bytes::from_static(&[0u8; 5])))
            .map_err(|e| e.to_string())?;
        let wreq = http_into_wasi_request(req).map_err(|e| format!("request: {e:?}"))?;
        let wresp = wasip3::http::client::send(wreq).await.map_err(|e| format!("send: {e:?}"))?;
        let resp = http_from_wasi_response(wresp).map_err(|e| format!("response: {e:?}"))?;
        let (parts, body) = resp.into_parts();
        let collected = body.collect().await.map_err(|e| format!("body: {e:?}"))?;
        let status = collected
            .trailers()
            .and_then(|t| t.get("grpc-status"))
            .or_else(|| parts.headers.get("grpc-status"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if status.as_deref() != Some("0") {
            return Err(format!("grpc-status {status:?}, http {}", parts.status));
        }
        let data = collected.to_bytes();
        if data.len() < 5 {
            return Err(format!("short gRPC frame: {} bytes", data.len()));
        }
        let msg = ListProductsResponse::decode(&data[5..]).map_err(|e| e.to_string())?;
        Ok(msg.products.len())
    }
}

// ------------------------------------------------------------ redis over sockets ---

struct WasiDbClient {
    addr: String,
}

fn parse_host_port(addr: &str) -> Result<(String, u16), String> {
    let addr = addr.strip_prefix("redis://").unwrap_or(addr).trim_end_matches('/');
    match addr.rfind(':') {
        Some(i) => Ok((addr[..i].to_string(), addr[i + 1..].parse().map_err(|_| format!("bad port in {addr}"))?)),
        None => Ok((addr.to_string(), 6379)),
    }
}

async fn resolve(host: &str) -> Result<IpAddress, String> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(match ip {
            std::net::IpAddr::V4(v) => {
                let o = v.octets();
                IpAddress::Ipv4((o[0], o[1], o[2], o[3]))
            }
            std::net::IpAddr::V6(v) => {
                let s = v.segments();
                IpAddress::Ipv6((s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]))
            }
        });
    }
    resolve_addresses(host.to_string())
        .await
        .map_err(|e| format!("resolve {host}: {e:?}"))?
        .into_iter()
        .next()
        .ok_or_else(|| format!("no address for {host}"))
}

fn resp_encode(args: &[&[u8]]) -> Vec<u8> {
    let mut buf = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        buf.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        buf.extend_from_slice(arg);
        buf.extend_from_slice(b"\r\n");
    }
    buf
}

fn resp_is_complete(buf: &[u8]) -> bool {
    let crlf = |b: &[u8]| b.windows(2).position(|w| w == b"\r\n");
    match buf.first() {
        None => false,
        Some(b'$') => match crlf(buf) {
            Some(pos) => match std::str::from_utf8(&buf[1..pos]).unwrap_or("").parse::<i64>() {
                Ok(len) if len < 0 => true,
                Ok(len) => buf.len() >= pos + 2 + len as usize + 2,
                Err(_) => false,
            },
            None => false,
        },
        Some(_) => crlf(buf).is_some(),
    }
}

#[tonic::async_trait]
impl DbClient for WasiDbClient {
    async fn roundtrip(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let r = self.roundtrip_inner(key, value).await;
        if let Err(e) = &r {
            eprintln!("redis {}: {e}", self.addr);
        }
        r
    }
}

impl WasiDbClient {
    async fn roundtrip_inner(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let (host, port) = parse_host_port(&self.addr)?;
        let (family, remote) = match resolve(&host).await? {
            IpAddress::Ipv4(address) => (IpAddressFamily::Ipv4, IpSocketAddress::Ipv4(Ipv4SocketAddress { port, address })),
            IpAddress::Ipv6(address) => (
                IpAddressFamily::Ipv6,
                IpSocketAddress::Ipv6(Ipv6SocketAddress { port, flow_info: 0, address, scope_id: 0 }),
            ),
        };
        let sock = TcpSocket::create(family).map_err(|e| format!("socket: {e:?}"))?;
        sock.connect(remote).await.map_err(|e| format!("connect {}: {e:?}", self.addr))?;
        let (mut tx, tx_reader) = wit_stream::new::<u8>();
        let _sending = sock.send(tx_reader);
        let (mut rx, _receiving) = sock.receive();
        for cmd in [resp_encode(&[b"SET", key.as_bytes(), value]), resp_encode(&[b"GET", key.as_bytes()])] {
            let rest = tx.write_all(cmd).await;
            if !rest.is_empty() {
                return Err("redis: short write".into());
            }
            let mut reply = Vec::new();
            loop {
                let (_status, chunk) = rx.read(Vec::with_capacity(4096)).await;
                if chunk.is_empty() {
                    return Err("redis: connection closed".into());
                }
                reply.extend_from_slice(&chunk);
                if resp_is_complete(&reply) {
                    break;
                }
            }
            if reply.first() == Some(&b'-') {
                return Err(format!("redis: {}", String::from_utf8_lossy(&reply[1..]).trim()));
            }
        }
        drop(tx);
        Ok(())
    }
}

// ------------------------------------------------------------------- service ---

struct Svc {
    net: WasiNetworkClient,
    db: WasiDbClient,
}

impl Svc {
    fn from_env() -> Self {
        Svc {
            net: WasiNetworkClient {
                addr: std::env::var("PRODUCT_CATALOG_SERVICE_ADDR").unwrap_or_else(|_| "localhost:3550".into()),
            },
            db: WasiDbClient { addr: std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis-cart:6379".into()) },
        }
    }

    async fn workload(&self) -> (HashMap<String, String>, i64) {
        let config = WorkloadConfig::from_env();
        let start = std::time::Instant::now();
        let results = core::run_workload(&config, &self.net, &self.db).await;
        (results, start.elapsed().as_millis() as i64)
    }
}

#[tonic::async_trait]
impl SyntheticService for Svc {
    async fn run_workload(
        &self,
        _request: tonic::Request<WorkloadRequest>,
    ) -> Result<tonic::Response<WorkloadResponse>, tonic::Status> {
        let (stressor_results, total_ms) = self.workload().await;
        Ok(tonic::Response::new(WorkloadResponse { stressor_results, total_ms }))
    }
}

fn json(results: &HashMap<String, String>, total_ms: i64) -> String {
    let mut s = String::from("{\"stressor_results\":{");
    for (i, (k, v)) in results.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{}\":\"{}\"", k.escape_default(), v.escape_default()));
    }
    s.push_str(&format!("}},\"total_ms\":{total_ms}}}"));
    s
}

struct Component;

impl wasip3::exports::http::handler::Guest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let req = http_from_wasi_request(request)?;
        let (parts, body) = req.into_parts();
        let bytes = body.collect().await.map_err(|_| ErrorCode::InternalError(None))?.to_bytes();
        let grpc = parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("application/grpc"))
            .unwrap_or(false);
        if grpc {
            let health = parts.uri.path().starts_with("/grpc.health.v1.Health/");
            let req = http::Request::from_parts(parts, Full::new(bytes));
            let resp = if health {
                let (_reporter, mut svc) = tonic_health::server::health_reporter();
                tonic::codegen::Service::call(&mut svc, req).await
            } else {
                let mut svc = SyntheticServiceServer::new(Svc::from_env());
                tonic::codegen::Service::call(&mut svc, req).await
            }
            .map_err(|_| ErrorCode::InternalError(None))?;
            return http_into_wasi_response(resp);
        }
        let (status, body) = if parts.method == http::Method::POST && parts.uri.path() == "/workload" {
            let (results, total_ms) = Svc::from_env().workload().await;
            (200, json(&results, total_ms))
        } else {
            (404, "{\"error\":\"not found\"}".to_string())
        };
        let resp = http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .header("content-length", body.len())
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| ErrorCode::InternalError(None))?;
        http_into_wasi_response(resp)
    }
}

wasip3::http::service::export!(Component);
