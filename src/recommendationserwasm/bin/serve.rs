use anyhow::{bail, Context, Result};
use wasmtime::{
    component::{Component, Linker, ResourceTable},
    Config, Engine, Store,
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{
    bindings::http::types::Scheme,
    body::HyperOutgoingBody,
    io::TokioIo,
    types::{HostFutureIncomingResponse, OutgoingRequestConfig},
    HttpResult, WasiHttpCtx, WasiHttpView,
};

wasmtime::component::bindgen!({
    world: "service",
    path: "../../wit",
    async: {
        only_imports: ["nonexistent"],
    },
    with: {
        "wasi:http": wasmtime_wasi_http::bindings::http,
        "wasi": wasmtime_wasi::bindings,
    },
    trappable_imports: true,
    require_store_data_send: true,
});
use tokio::net::TcpListener;
use hyper::{body::Incoming, Request, Response};
use hyper::service::service_fn;
use hyper_util::rt::TokioExecutor;
use hyper_util::server::conn::auto::Builder;

// Define the host state that implements the required WASI traits
struct Host {
    table: ResourceTable,
    ctx: WasiCtx,
    http: WasiHttpCtx,
}

impl WasiView for Host {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}

impl WasiHttpView for Host {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    /// Override send_request to use HTTP/2 prior knowledge (h2c) for outgoing
    /// requests, which is required for communicating with gRPC servers.
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        Ok(h2c_send_request(request, config))
    }
}

#[derive(Clone)]
struct Server {
    pre: ServicePre<Host>,
}

impl Server {
    async fn handle_request(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<HyperOutgoingBody>> {
        println!("Incoming request: {} {}", req.method(), req.uri());
        // Create per-http-request state within a Store
        let mut store = Store::new(
            self.pre.engine(),
            Host {
                table: ResourceTable::new(),
                ctx: WasiCtxBuilder::new().inherit_stdio().inherit_env().build(),
                http: WasiHttpCtx::new(),
            },
        );

        // Create a oneshot channel for the response
        let (sender, receiver) = tokio::sync::oneshot::channel();

        // Convert the request into the WASI HTTP types
        let req = store
            .data_mut()
            .new_incoming_request(Scheme::Http, req)?;
        
        let out = store.data_mut().new_response_outparam(sender)?;

        let pre = self.pre.clone();

        // Spawn a task to handle the request
        let task = tokio::task::spawn(async move {
            let proxy = pre.instantiate_async(&mut store).await?;

            if let Err(e) = proxy
                .wasi_http_incoming_handler()
                .call_handle(&mut store, req, out)
                .await
            {
                return Err(e);
            }

            Ok(())
        });

        // Wait for the response
        match receiver.await {
            Ok(Ok(resp)) => Ok(resp),

            Ok(Err(e)) => Err(e.into()),

            // Otherwise the sender got dropped, inspect the task result
            Err(_) => {
                let e = match task.await {
                    Ok(Ok(())) => {
                        bail!("guest never invoked `response-outparam::set` method")
                    }
                    Ok(Err(e)) => e,
                    Err(e) => e.into(),
                };
                Err(e.context("guest never invoked `response-outparam::set` method"))
            }
        }
    }
}

/// Send an outgoing HTTP request using HTTP/2 prior knowledge (h2c).
/// This is required because gRPC servers only speak HTTP/2.
fn h2c_send_request(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> HostFutureIncomingResponse {
    let handle = wasmtime_wasi::runtime::spawn(async move {
        Ok(h2c_send_request_handler(request, config).await)
    });
    HostFutureIncomingResponse::pending(handle)
}

async fn h2c_send_request_handler(
    mut request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> Result<wasmtime_wasi_http::types::IncomingResponse, wasmtime_wasi_http::bindings::http::types::ErrorCode> {
    use wasmtime_wasi_http::bindings::http::types as wasi_types;
    use wasmtime_wasi_http::types::IncomingResponse;
    use http_body_util::BodyExt;
    use tokio::time::timeout;

    let authority = if let Some(authority) = request.uri().authority() {
        if authority.port().is_some() {
            authority.to_string()
        } else {
            let port = if config.use_tls { 443 } else { 80 };
            format!("{}:{port}", authority)
        }
    } else {
        return Err(wasi_types::ErrorCode::HttpRequestUriInvalid);
    };

    let tcp_stream = timeout(config.connect_timeout, tokio::net::TcpStream::connect(&authority))
        .await
        .map_err(|_| wasi_types::ErrorCode::ConnectionTimeout)?
        .map_err(|_| wasi_types::ErrorCode::ConnectionRefused)?;

    let tcp_stream = TokioIo::new(tcp_stream);

    // Use HTTP/2 prior knowledge (h2c) handshake
    let (mut sender, conn) = timeout(
        config.connect_timeout,
        hyper::client::conn::http2::handshake(TokioExecutor::new(), tcp_stream),
    )
    .await
    .map_err(|_| wasi_types::ErrorCode::ConnectionTimeout)?
    .map_err(|e| {
        eprintln!("h2c handshake error: {:?}", e);
        wasi_types::ErrorCode::HttpProtocolError
    })?;

    let worker = wasmtime_wasi::runtime::spawn(async move {
        match conn.await {
            Ok(()) => {}
            Err(e) => eprintln!("h2 connection error: {:?}", e),
        }
    });

    // Strip scheme and authority from URI for the request
    *request.uri_mut() = http::Uri::builder()
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/"),
        )
        .build()
        .expect("comes from valid request");

    let resp = timeout(config.first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| wasi_types::ErrorCode::ConnectionReadTimeout)?
        .map_err(|e| {
            eprintln!("h2 send_request error: {:?}", e);
            wasi_types::ErrorCode::HttpProtocolError
        })?
        .map(|body| body.map_err(|e| {
            eprintln!("h2 body error: {:?}", e);
            wasi_types::ErrorCode::HttpProtocolError
        }).boxed());

    Ok(IncomingResponse {
        resp,
        worker: Some(worker),
        between_bytes_timeout: config.between_bytes_timeout,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Starting RecommendationService WASM server...");

    // Configure Wasmtime engine
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);

    let engine = Engine::new(&config)?;

    // Load the WASM component
    let wasm_path = "./recommendationserwasm.wasm";
    println!("Loading WASM component from: {}", wasm_path);
    let component = Component::from_file(&engine, wasm_path)
        .context("Failed to load WASM component")?;

    println!("WASM component loaded successfully");

    // Create linker
    let mut linker = Linker::new(&engine);
    
    // Add WASI P2 and HTTP support to the linker
    wasmtime_wasi::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

    // Pre-instantiate with custom service world (superset of proxy)
    let pre = ServicePre::new(linker.instantiate_pre(&component)?)?;
    let server = Server { pre };

    // Bind to the address
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    println!("Server listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        
        let server_clone = server.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let server = server_clone.clone();
                async move { server.handle_request(req).await }
            });

            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}
