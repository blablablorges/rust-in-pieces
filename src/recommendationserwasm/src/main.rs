use tonic::{Request, Response, Status};
use prost::Message;

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

use hipstershop::recommendation_service_server::{RecommendationService, RecommendationServiceServer};
use hipstershop::{ListRecommendationsRequest, ListRecommendationsResponse, Empty, ListProductsResponse};

#[wasi_grpc_server::grpc_component(RecommendationServiceServer)]
struct RecommendationServiceImpl;

#[tonic::async_trait]
impl RecommendationService for RecommendationServiceImpl {
    async fn list_recommendations(
        &self,
        request: Request<ListRecommendationsRequest>,
    ) -> Result<Response<ListRecommendationsResponse>, Status> {
        let req = request.get_ref();

        // Call ProductCatalogService to get all products
        let catalog_addr = std::env::var("PRODUCT_CATALOG_SERVICE_ADDR")
            .unwrap_or_else(|_| "localhost:3550".to_string());

        let products = match call_list_products(&catalog_addr) {
            Ok(p) => p,
            Err(e) => {
                println!("[ERROR] Failed to call product catalog: {}", e);
                return Err(Status::internal(format!("Failed to call product catalog: {}", e)));
            }
        };

        // Get all product IDs
        let product_ids: Vec<String> = products.products.iter().map(|p| p.id.clone()).collect();

        // Filter out products already in the request
        let filtered: Vec<String> = product_ids
            .into_iter()
            .filter(|id| !req.product_ids.contains(id))
            .collect();

        // Randomly select up to 5 products
        let max_responses = 5;
        let num_return = std::cmp::min(max_responses, filtered.len());
        let indices = random_sample_indices(filtered.len(), num_return);
        let selected: Vec<String> = indices.into_iter().map(|i| filtered[i].clone()).collect();

        println!("[Recv ListRecommendations] product_ids={:?}", selected);

        Ok(Response::new(ListRecommendationsResponse {
            product_ids: selected,
        }))
    }
}

/// Make an outgoing gRPC call to ProductCatalogService.ListProducts
/// using the WASI HTTP outgoing handler.
fn call_list_products(addr: &str) -> Result<ListProductsResponse, String> {
    // Encode Empty message as protobuf
    let empty = Empty {};
    let mut proto_buf = Vec::new();
    empty.encode(&mut proto_buf).map_err(|e| format!("encode error: {}", e))?;

    // gRPC framing: 1 byte compressed flag (0) + 4 bytes big-endian length + protobuf message
    let mut grpc_frame = Vec::with_capacity(5 + proto_buf.len());
    grpc_frame.push(0u8);
    grpc_frame.extend_from_slice(&(proto_buf.len() as u32).to_be_bytes());
    grpc_frame.extend_from_slice(&proto_buf);

    // Build outgoing HTTP request headers
    let headers = wasi::http::types::Fields::new();
    headers.append(&"content-type".into(), &b"application/grpc".to_vec())
        .map_err(|e| format!("set content-type error: {:?}", e))?;

    let request = wasi::http::types::OutgoingRequest::new(headers);
    request.set_method(&wasi::http::types::Method::Post)
        .map_err(|()| "set method failed".to_string())?;
    request.set_scheme(Some(&wasi::http::types::Scheme::Http))
        .map_err(|()| "set scheme failed".to_string())?;
    request.set_authority(Some(addr))
        .map_err(|()| "set authority failed".to_string())?;
    request.set_path_with_query(Some("/hipstershop.ProductCatalogService/ListProducts"))
        .map_err(|()| "set path failed".to_string())?;

    // Get body and write stream BEFORE calling handle
    let body = request.body()
        .map_err(|_| "get body failed".to_string())?;
    let stream = body.write()
        .map_err(|_| "get write stream failed".to_string())?;

    // Send the request via WASI outgoing handler
    let future_response = wasi::http::outgoing_handler::handle(request, None)
        .map_err(|e| format!("outgoing handle error: {:?}", e))?;

    // Write the gRPC request body
    stream.blocking_write_and_flush(&grpc_frame)
        .map_err(|e| format!("write error: {:?}", e))?;
    drop(stream);
    wasi::http::types::OutgoingBody::finish(body, None)
        .map_err(|e| format!("finish body error: {:?}", e))?;

    // Wait for the response
    let pollable = future_response.subscribe();
    pollable.block();

    let response = future_response.get()
        .ok_or_else(|| "no response ready".to_string())?
        .map_err(|_| "future error".to_string())?
        .map_err(|e| format!("response error: {:?}", e))?;

    let status = response.status();
    if status != 200 {
        return Err(format!("unexpected HTTP status: {}", status));
    }

    // Read the response body
    let incoming_body = response.consume()
        .map_err(|_| "consume body failed".to_string())?;
    let input_stream = incoming_body.stream()
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

    // Decode gRPC response frame
    if response_bytes.len() < 5 {
        return Err(format!("response too short: {} bytes", response_bytes.len()));
    }

    let _compressed = response_bytes[0];
    let msg_len = u32::from_be_bytes([
        response_bytes[1], response_bytes[2], response_bytes[3], response_bytes[4],
    ]) as usize;

    if response_bytes.len() < 5 + msg_len {
        return Err(format!(
            "incomplete response: expected {} bytes, got {}",
            5 + msg_len,
            response_bytes.len()
        ));
    }

    let proto_bytes = &response_bytes[5..5 + msg_len];
    ListProductsResponse::decode(proto_bytes)
        .map_err(|e| format!("decode error: {}", e))
}

/// Randomly sample `count` indices from `0..total` using Fisher-Yates partial shuffle.
fn random_sample_indices(total: usize, count: usize) -> Vec<usize> {
    if count >= total {
        return (0..total).collect();
    }

    let mut indices = Vec::with_capacity(count);
    let mut available: Vec<usize> = (0..total).collect();

    for _ in 0..count {
        let idx = fastrand::usize(0..available.len());
        indices.push(available.swap_remove(idx));
    }

    indices
}
