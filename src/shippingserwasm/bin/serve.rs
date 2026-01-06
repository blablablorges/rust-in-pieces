use anyhow::{bail, Context, Result};
use wasmtime::{
    component::{Component, Linker, ResourceTable},
    Config, Engine, Store,
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{
    bindings::ProxyPre,
    bindings::http::types::Scheme,
    body::HyperOutgoingBody,
    io::TokioIo,
    WasiHttpCtx, WasiHttpView,
};
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
}

#[derive(Clone)]
struct Server {
    pre: ProxyPre<Host>,
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
        println!("Invoking WASM component handler...");


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
            // If the guest calls `response-outparam::set` then we get the response
            Ok(Ok(resp)) => {
/*                 println!("✅ Response from WASM: status {}", resp.status()); */
                Ok(resp)
            }

            Ok(Err(e)) => Err(e.into()),

            // Otherwise the sender got dropped, inspect the task result
            Err(_) => {
                let e = match task.await {
                    Ok(Ok(())) => {
/*                         println!("❌ Guest never invoked response-outparam::set"); */
                        bail!("guest never invoked `response-outparam::set` method")
                    }
                    Ok(Err(e)) => {
/*                         println!("❌ Error in WASM task: {:?}", e);
 */                        e
                    }
                    Err(e) => {
/*                         println!("❌ Task join error: {:?}", e);
 */                        e.into()
                    }
                };
                Err(e.context("guest never invoked `response-outparam::set` method"))
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Starting custom Wasmtime server...");

    // Configure Wasmtime engine
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);

    let engine = Engine::new(&config)?;

    // Load the WASM component
    let wasm_path = "./shippingserwasm.wasm";
    println!("Loading WASM component from: {}", wasm_path);
    let component = Component::from_file(&engine, wasm_path)
        .context("Failed to load WASM component")?;

    println!("WASM component loaded successfully");

    // Create linker
    let mut linker = Linker::new(&engine);
    
    // Add WASI P2 and HTTP support to the linker
    wasmtime_wasi::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

    // Pre-instantiate the proxy
    let pre = ProxyPre::new(linker.instantiate_pre(&component)?)?;
    let server = Server { pre };

    // Bind to the address
    let addr = "shippingservice:50051";
    let listener = TcpListener::bind(addr).await?;
    println!("Server listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
/*         println!("New connection");
 */
        let io = TokioIo::new(stream);
        
        let server_clone = server.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let server = server_clone.clone();
                async move { server.handle_request(req).await }
            });

            if let Err(_err) = Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
/*                 eprintln!("Error serving connection: {:?}", err);
 */            }  else {
/*                 println!("Connection closed successfully");
 */            }
        });
    }
}