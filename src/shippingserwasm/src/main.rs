
use tonic::{Request, Response, Status};

pub mod hipstershop {
    tonic::include_proto!("hipstershop");
}

use hipstershop::shipping_service_server::{ShippingService, ShippingServiceServer};
use hipstershop::{GetQuoteRequest, GetQuoteResponse, ShipOrderRequest, ShipOrderResponse};
use hipstershop::{Money};

const NANOS_MULTIPLE: u32 = 10000000u32;


#[wasi_grpc_server::grpc_component(ShippingServiceServer)]
struct ShippingServiceImpl;

#[tonic::async_trait]
impl ShippingService for ShippingServiceImpl {



    async fn get_quote(&self, request: Request<GetQuoteRequest>) -> Result<Response<GetQuoteResponse>, Status> {
/*         println!("Received GetQuoteRequest");
 */
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
/*         println!("Received ShipOrderRequest");
 */
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

fn create_quote_from_count(_count: u32) -> Quote {
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


fn create_tracking_id(_salt: &str) -> String {
    String::from("AB-25123-121234567")
/*     format!(
        "{}{}-{}{}-{}{}",
        get_random_letter_code(),
        get_random_letter_code(),
        salt.len(),
        get_random_number(3),
        salt.len() / 2,
        get_random_number(7)
    ) */
}

/* fn get_random_letter_code() -> char {
    let code = 65 + fastrand::u32(0..26);
    char::from_u32(code).unwrap()
}

fn get_random_number(digits: usize) -> String {
    (0..digits)
        .map(|_| fastrand::u32(0..10).to_string())
        .collect()
} */