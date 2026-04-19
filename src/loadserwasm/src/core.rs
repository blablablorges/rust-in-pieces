use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum Level {
    None,
    Mid,
    High,
}

impl Level {
    pub fn from_env(var: &str) -> Self {
        match std::env::var(var).as_deref() {
            Ok("1") => Level::Mid,
            Ok("2") => Level::High,
            _ => Level::None,
        }
    }
}

pub struct WorkloadConfig {
    pub cpu: Level,
    pub memory: Level,
    pub io: Level,
    pub network: Level,
    pub db: Level,
}

impl WorkloadConfig {
    pub fn from_env() -> Self {
        Self {
            cpu: Level::from_env("CPU_LEVEL"),
            memory: Level::from_env("MEMORY_LEVEL"),
            io: Level::from_env("IO_LEVEL"),
            network: Level::from_env("NETWORK_LEVEL"),
            db: Level::from_env("DB_LEVEL"),
        }
    }
}

// ---------------------------------------------------------------------------
// I/O traits
// WASM impls live in main.rs (WASI HTTP / WASI TCP).
// Native impls live in server.rs (tonic / redis crate).
// ---------------------------------------------------------------------------

#[tonic::async_trait]
pub trait NetworkClient: Send + Sync {
    /// Make one ListProducts call. Returns the product count on success.
    async fn list_products_once(&self) -> Result<usize, String>;
}

#[tonic::async_trait]
pub trait DbClient: Send + Sync {
    /// One SET followed by one GET roundtrip.
    async fn roundtrip(&self, key: &str, value: &[u8]) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Pure stressors (no I/O — compile and run identically on WASM and native)
// ---------------------------------------------------------------------------

pub fn run_cpu(level: Level) -> String {
    let limit = match level {
        Level::None => return "skipped".into(),
        Level::Mid => 10_000,
        Level::High => 200_000,
    };
    let count = prime_sieve(limit);
    format!("found {} primes ≤ {}", count, limit)
}

pub fn run_memory(level: Level) -> String {
    let size: usize = match level {
        Level::None => return "skipped".into(),
        Level::Mid => 16 * 1024 * 1024,
        Level::High => 128 * 1024 * 1024,
    };
    // Allocate and touch every 4 KiB page to force physical allocation.
    let mut buf: Vec<u8> = vec![0xABu8; size];
    let mut checksum: u64 = 0;
    for chunk in buf.chunks(4096) {
        checksum = checksum.wrapping_add(chunk[0] as u64);
    }
    buf[0] = (checksum & 0xFF) as u8; // prevent dead-code elimination
    drop(buf);
    format!("allocated and touched {} MiB", size / (1024 * 1024))
}

/// Uses std::fs which maps to WASI filesystem on wasm32-wasip2.
/// Requires the host to pre-open WORKLOAD_TMP_DIR (default /tmp).
/// Fails gracefully if the filesystem interface is unavailable.
pub fn run_io(level: Level) -> String {
    let size: usize = match level {
        Level::None => return "skipped".into(),
        Level::Mid => 1 * 1024 * 1024,
        Level::High => 16 * 1024 * 1024,
    };
    let dir = std::env::var("WORKLOAD_TMP_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let path = format!("{}/loadserwasm-probe", dir);
    let data: Vec<u8> = vec![0xABu8; size];
    if let Err(e) = std::fs::write(&path, &data) {
        return format!("write error: {}", e);
    }
    match std::fs::read(&path) {
        Ok(read_data) => {
            let _ = std::fs::remove_file(&path);
            format!("wrote+read {} MiB", read_data.len() / (1024 * 1024))
        }
        Err(e) => format!("read error: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Async stressors (delegate I/O through the injected traits)
// ---------------------------------------------------------------------------

pub async fn run_network<C: NetworkClient>(client: &C, level: Level) -> String {
    let calls: usize = match level {
        Level::None => return "skipped".into(),
        Level::Mid => 3,
        Level::High => 10,
    };
    let mut total = 0usize;
    let mut errors = 0usize;
    for _ in 0..calls {
        match client.list_products_once().await {
            Ok(n) => total += n,
            Err(_) => errors += 1,
        }
    }
    format!("{} calls, {} products, {} errors", calls, total, errors)
}

pub async fn run_db<D: DbClient>(client: &D, level: Level) -> String {
    let rounds: usize = match level {
        Level::None => return "skipped".into(),
        Level::Mid => 5,
        Level::High => 20,
    };
    let mut errors = 0usize;
    for i in 0..rounds {
        let key = format!("synthetic:probe:{}", i);
        if client.roundtrip(&key, b"loadserwasm").await.is_err() {
            errors += 1;
        }
    }
    format!("{} roundtrips, {} errors", rounds, errors)
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

pub async fn run_workload<N: NetworkClient, D: DbClient>(
    config: &WorkloadConfig,
    network: &N,
    db: &D,
) -> HashMap<String, String> {
    let mut results = HashMap::new();
    results.insert("cpu".into(), run_cpu(config.cpu));
    results.insert("memory".into(), run_memory(config.memory));
    results.insert("io".into(), run_io(config.io));
    results.insert("network".into(), run_network(network, config.network).await);
    results.insert("db".into(), run_db(db, config.db).await);
    results
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn prime_sieve(limit: usize) -> usize {
    if limit < 2 {
        return 0;
    }
    let mut sieve = vec![true; limit + 1];
    sieve[0] = false;
    sieve[1] = false;
    let mut i = 2;
    while i * i <= limit {
        if sieve[i] {
            let mut j = i * i;
            while j <= limit {
                sieve[j] = false;
                j += i;
            }
        }
        i += 1;
    }
    sieve.iter().filter(|&&x| x).count()
}
