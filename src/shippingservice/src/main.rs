
use tonic::{Request, Response, Status};
use rand::Rng;
use tonic::transport::Server;
use std::env;

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

use hipstershop::shipping_service_server::{ShippingService, ShippingServiceServer};
use hipstershop::{GetQuoteRequest, GetQuoteResponse, ShipOrderRequest, ShipOrderResponse};
use hipstershop::{Money};

const NANOS_MULTIPLE: u32 = 10000000u32;


#[derive(Debug)]
struct ShippingServiceImpl;

#[tonic::async_trait]
impl ShippingService for ShippingServiceImpl {
    async fn get_quote(&self, request: Request<GetQuoteRequest>) -> Result<Response<GetQuoteResponse>, Status> {
        println!("Received GetQuoteRequest");

        let itemcount: u32 = request.get_ref().items.iter().map(|item| item.quantity as u32).sum();

        let quote = create_quote_from_count(itemcount);

        let quote_response = GetQuoteResponse {
            cost_usd: Some(Money {
                currency_code: "USD".into(),
                units: quote.dollars as i64,
                nanos: quote.cents as i32 * NANOS_MULTIPLE as i32,
            }),
        };
        Ok(Response::new(quote_response))
    }

    async fn ship_order(&self, request: Request<ShipOrderRequest>) -> Result<Response<ShipOrderResponse>, Status> {
        println!("Received ShipOrderRequest");

        let address = request.get_ref().address.as_ref()
            .ok_or_else(|| Status::invalid_argument("Address is required"))?;
    
        let base_address = format!(
            "{}, {}, {}",
            address.street_address,
            address.city,
            address.state
        );
    
        let tid = create_tracking_id(&base_address);
    
        let shipped_response = ShipOrderResponse {
            tracking_id: tid,
        };
    
        Ok(Response::new(shipped_response))
        
    }
}

struct Quote {
    dollars: i64,
    cents: i32,
}

fn create_quote_from_count(count: u32) -> Quote {
    create_quote_from_float(8.99)
}

fn create_quote_from_float(value: f64) -> Quote {
    let units = value.trunc();
    let fraction = value.fract();
    
    Quote {
        dollars: units as i64,
        cents: (fraction * 100.0).trunc() as i32,
    }
}


fn create_tracking_id(salt: &str) -> String {
    let mut rng = rand::thread_rng();
    
    format!(
        "{}{}-{}{}-{}{}",
        get_random_letter_code(&mut rng),
        get_random_letter_code(&mut rng),
        salt.len(),
        get_random_number(&mut rng, 3),
        salt.len() / 2,
        get_random_number(&mut rng, 7)
    )
}

fn get_random_letter_code(rng: &mut impl Rng) -> char {
    let code = 65 + rng.gen_range(0..26);
    char::from_u32(code).unwrap()
}

fn get_random_number(rng: &mut impl Rng, digits: usize) -> String {
    (0..digits)
        .map(|_| rng.gen_range(0..10).to_string())
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = env::var("SHIPPING_PORT")
        .or_else(|_| env::var("PORT"))
        .unwrap_or_else(|_| "50051".to_string());
    
    let addr = format!("0.0.0.0:{}", port).parse()?;

    let shipping = ShippingServiceImpl;

    println!("ShippingService server listening on {}", addr);

    let svc = ShippingServiceServer::new(shipping);

    Server::builder().add_service(svc).serve(addr).await?;

    Ok(())
}