/// Computed shipping cost.
pub struct Quote {
    pub dollars: i64,
    pub cents: i32,
}

/// Returns a fixed shipping quote regardless of item count.
pub fn compute_quote(_item_count: u32) -> Quote {
    quote_from_float(8.99)
}

fn quote_from_float(value: f64) -> Quote {
    Quote {
        dollars: value.trunc() as i64,
        cents: (value.fract() * 100.0).trunc() as i32,
    }
}

/// Generates a random tracking ID seeded by the destination address.
pub fn create_tracking_id(salt: &str) -> String {
    let quote_id = 1_000_000 + fastrand::u32(0..899_999);
    format!(
        "{}{}-{}{}-{}{}",
        "ISE",
        random_letter(),
        salt.len(),
        random_digits(3),
        salt.len() / 2,
        quote_id
    )
}

fn random_letter() -> char {
    char::from_u32(65 + fastrand::u32(0..26)).unwrap()
}

fn random_digits(n: usize) -> String {
    (0..n).map(|_| fastrand::u32(0..10).to_string()).collect()
}
