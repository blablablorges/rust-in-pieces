use tonic::Status;

use super::hipstershop::{ListRecommendationsRequest, ListRecommendationsResponse};

/// Catalog adapter — implemented by the WASI HTTP outgoing backend and the
/// native tonic gRPC client backend.
#[tonic::async_trait]
pub trait CatalogClient: Send + Sync {
    async fn list_product_ids(&self) -> Result<Vec<String>, String>;
}

pub async fn list_recommendations<C: CatalogClient>(
    client: &C,
    req: &ListRecommendationsRequest,
) -> Result<ListRecommendationsResponse, Status> {
    let all_ids = client
        .list_product_ids()
        .await
        .map_err(|e| Status::internal(format!("Failed to call product catalog: {}", e)))?;

    let filtered: Vec<String> = all_ids
        .into_iter()
        .filter(|id| !req.product_ids.contains(id))
        .collect();

    let num_return = std::cmp::min(5, filtered.len());
    let indices = random_sample_indices(filtered.len(), num_return);
    let selected: Vec<String> = indices.into_iter().map(|i| filtered[i].clone()).collect();

    println!("[Recv ListRecommendations] returning product_ids={:?}", selected);

    Ok(ListRecommendationsResponse {
        product_ids: selected,
    })
}

fn random_sample_indices(total: usize, count: usize) -> Vec<usize> {
    if count >= total {
        return (0..total).collect();
    }
    let mut available: Vec<usize> = (0..total).collect();
    let mut indices = Vec::with_capacity(count);
    for _ in 0..count {
        let idx = fastrand::usize(0..available.len());
        indices.push(available.swap_remove(idx));
    }
    indices
}
