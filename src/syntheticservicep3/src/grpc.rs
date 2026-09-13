//! One unary gRPC call over the host's wasi:http 0.3 client (h2 prior-knowledge to the
//! authorities in WASIHOST_H2C_AUTHORITIES). Request and response are single 5-byte-framed
//! protobuf messages; the status comes from the `grpc-status` trailer.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use prost::Message;
use wasip3::http_compat::{http_from_wasi_response, http_into_wasi_request};

pub async fn unary<Req: Message, Resp: Message + Default>(
    authority: &str,
    path: &str,
    req: &Req,
) -> Result<Resp, String> {
    let mut frame = vec![0u8; 5];
    req.encode(&mut frame).map_err(|e| e.to_string())?;
    let len = (frame.len() - 5) as u32;
    frame[1..5].copy_from_slice(&len.to_be_bytes());
    let hreq = http::Request::builder()
        .method("POST")
        .uri(format!("http://{authority}{path}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Full::new(Bytes::from(frame)))
        .map_err(|e| e.to_string())?;
    let wreq = http_into_wasi_request(hreq).map_err(|e| format!("request: {e:?}"))?;
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
    Resp::decode(&data[5..]).map_err(|e| e.to_string())
}
