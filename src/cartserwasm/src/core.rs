use prost::Message;
use tonic::Status;

use super::hipstershop::{AddItemRequest, Cart, CartItem, Empty};

/// Storage adapter — implemented by the WASI TCP/RESP backend and the native
/// Redis backend.  Operates on raw protobuf bytes so the core never touches
/// transport-specific types.
#[tonic::async_trait]
pub trait CartStore: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    async fn save(&self, key: &str, data: Vec<u8>) -> Result<(), String>;
}

pub fn cart_key(user_id: &str) -> String {
    format!("cart:{}", user_id)
}

pub async fn add_item<S: CartStore>(store: &S, req: AddItemRequest) -> Result<Empty, Status> {
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

    let key = cart_key(&user_id);
    let mut cart = load_cart(store, &user_id, &key).await?;

    if let Some(existing) = cart
        .items
        .iter_mut()
        .find(|i| i.product_id == item.product_id)
    {
        existing.quantity += item.quantity;
    } else {
        cart.items.push(CartItem {
            product_id: item.product_id,
            quantity: item.quantity,
        });
    }

    save_cart(store, &key, &cart).await?;
    Ok(Empty {})
}

pub async fn get_cart<S: CartStore>(store: &S, user_id: String) -> Result<Cart, Status> {
    if user_id.is_empty() {
        return Err(Status::invalid_argument("user_id is required"));
    }
    load_cart(store, &user_id, &cart_key(&user_id)).await
}

pub async fn empty_cart<S: CartStore>(store: &S, user_id: String) -> Result<Empty, Status> {
    if user_id.is_empty() {
        return Err(Status::invalid_argument("user_id is required"));
    }
    let key = cart_key(&user_id);
    let empty = Cart {
        user_id: user_id.clone(),
        items: vec![],
    };
    save_cart(store, &key, &empty).await?;
    Ok(Empty {})
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn load_cart<S: CartStore>(
    store: &S,
    user_id: &str,
    key: &str,
) -> Result<Cart, Status> {
    match store.load(key).await {
        Ok(Some(bytes)) => Cart::decode(bytes.as_slice())
            .map_err(|e| Status::internal(format!("failed to decode cart: {}", e))),
        Ok(None) => Ok(Cart {
            user_id: user_id.to_string(),
            items: vec![],
        }),
        Err(e) => Err(Status::failed_precondition(format!(
            "can't access cart storage: {}",
            e
        ))),
    }
}

async fn save_cart<S: CartStore>(store: &S, key: &str, cart: &Cart) -> Result<(), Status> {
    let mut bytes = Vec::new();
    cart.encode(&mut bytes)
        .map_err(|e| Status::internal(format!("failed to encode cart: {}", e)))?;
    store
        .save(key, bytes)
        .await
        .map_err(|e| Status::failed_precondition(format!("can't access cart storage: {}", e)))
}
