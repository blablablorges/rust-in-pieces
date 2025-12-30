use tonic::Request;

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

use hipstershop::shipping_service_client::{ShippingServiceClient};
use hipstershop::{GetQuoteRequest, ShipOrderRequest, CartItem, Address};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ShippingServiceClient::connect("http://[::1]:50051").await?;

    println!("*** SIMPLE RPC ***");
    let response = client
        .ship_order(Request::new(ShipOrderRequest {
            address: Some(Address {
                city: "anim".to_string(),
                country: "Duis ad".to_string(),
                state: "Excepteur ipsum enim".to_string(),
                street_address: "eiusmod".to_string(),
                zip_code: 21111,
            }),
            items: vec![
                CartItem {
                    product_id: "et incididunt aliqua".to_string(),
                    quantity: 2,
                }
            ],
        }))
    .await?;

    println!("RESPONSE = {response:?}");

     Ok(())
}