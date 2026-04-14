use prost::Message;
use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

mod core;

use core::CatalogClient;
use hipstershop::recommendation_service_server::{RecommendationService, RecommendationServiceServer};
use hipstershop::{Empty, ListProductsResponse, ListRecommendationsRequest, ListRecommendationsResponse};

// ---------------------------------------------------------------------------
// WASI catalog adapter — makes an outgoing HTTP/2 gRPC call via wasi:http
// ---------------------------------------------------------------------------

struct WasiCatalogClient {
    addr: String,
}

#[tonic::async_trait]
impl CatalogClient for WasiCatalogClient {
    async fn list_product_ids(&self) -> Result<Vec<String>, String> {
        let resp = call_list_products(&self.addr)?;
        Ok(resp.products.into_iter().map(|p| p.id).collect())
    }
}

// ---------------------------------------------------------------------------
// gRPC component — delegates entirely to core logic
// ---------------------------------------------------------------------------

#[wasi_grpc_server::grpc_component(RecommendationServiceServer)]
struct RecommendationServiceImpl;

#[tonic::async_trait]
impl RecommendationService for RecommendationServiceImpl {
    async fn list_recommendations(
        &self,
        request: Request<ListRecommendationsRequest>,
    ) -> Result<Response<ListRecommendationsResponse>, Status> {
        let addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
            .unwrap_or_else(|_| "localhost:3550".to_string());
        let client = WasiCatalogClient { addr };
        core::list_recommendations(&client, request.get_ref())
            .await
            .map(Response::new)
    }
}

// ---------------------------------------------------------------------------
// WASI HTTP outgoing call to ProductCatalogService (WASI-specific, stays here)
// ---------------------------------------------------------------------------

fn call_list_products(addr: &str) -> Result<ListProductsResponse, String> {
    println!("[DEBUG] call_list_products: addr={}", addr);

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
        .map_err(|e| format!("set content-type error: {:?}", e))?;

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

    let body = request
        .body()
        .map_err(|_| "get body failed".to_string())?;
    let stream = body.write().map_err(|_| "get write stream failed".to_string())?;

    println!("[DEBUG] calling wasi::http::outgoing_handler::handle");
    let future_response = wasi::http::outgoing_handler::handle(request, None)
        .map_err(|e| format!("outgoing handle error: {:?}", e))?;

    stream
        .blocking_write_and_flush(&grpc_frame)
        .map_err(|e| format!("write error: {:?}", e))?;
    drop(stream);
    wasi::http::types::OutgoingBody::finish(body, None)
        .map_err(|e| format!("finish body error: {:?}", e))?;

    let pollable = future_response.subscribe();
    pollable.block();

    let response = future_response
        .get()
        .ok_or_else(|| "no response ready".to_string())?
        .map_err(|_| "future error".to_string())?
        .map_err(|e| format!("response error: {:?}", e))?;

    let status = response.status();
    if status != 200 {
        return Err(format!("unexpected HTTP status: {}", status));
    }

    let incoming_body = response
        .consume()
        .map_err(|_| "consume body failed".to_string())?;
    let input_stream = incoming_body
        .stream()
        .map_err(|_| "get input stream failed".to_string())?;

    let mut response_bytes = Vec::new();
    loop {
        match input_stream.blocking_read(65536) {
            Ok(chunk) => {
                if chunk.is_empty() {
                    break;
                }
                response_bytes.extend_from_slice(&chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(e) => return Err(format!("read error: {:?}", e)),
        }
    }
    drop(input_stream);

    if response_bytes.len() < 5 {
        return Err(format!(
            "response too short: {} bytes",
            response_bytes.len()
        ));
    }

    let msg_len = u32::from_be_bytes([
        response_bytes[1],
        response_bytes[2],
        response_bytes[3],
        response_bytes[4],
    ]) as usize;

    if response_bytes.len() < 5 + msg_len {
        return Err(format!(
            "incomplete response: expected {} bytes, got {}",
            5 + msg_len,
            response_bytes.len()
        ));
    }

    ListProductsResponse::decode(&response_bytes[5..5 + msg_len])
        .map_err(|e| format!("decode error: {}", e))
}
