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
    WasiHttpCtx, WasiHttpView,
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
    pre: ServicePre<Host>,
}

impl Server {
    async fn handle_request(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<HyperOutgoingBody>> {
        println!("Incoming request: {} {}", req.method(), req.uri());

        let file_perms = wasmtime_wasi::FilePerms::all();
        let dir_perms  = wasmtime_wasi::DirPerms::all();
        let mut store = Store::new(
            self.pre.engine(),
            Host {
                table: ResourceTable::new(),
                ctx: WasiCtxBuilder::new()
                    .inherit_stdio()
                    .inherit_env()
                    .inherit_network()
                    .allow_ip_name_lookup(true)
                    .preopened_dir("/tmp", "/tmp", dir_perms, file_perms)?
                    .build(),
                http: WasiHttpCtx::new(),
            },
        );

        let (sender, receiver) = tokio::sync::oneshot::channel();

        let req = store
            .data_mut()
            .new_incoming_request(Scheme::Http, req)?;

        let out = store.data_mut().new_response_outparam(sender)?;

        let pre = self.pre.clone();

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

        match receiver.await {
            Ok(Ok(resp)) => Ok(resp),

            Ok(Err(e)) => Err(e.into()),

            Err(_) => {
                let e = match task.await {
                    Ok(Ok(())) => bail!("guest never invoked `response-outparam::set` method"),
                    Ok(Err(e)) => e,
                    Err(e) => e.into(),
                };
                Err(e.context("guest never invoked `response-outparam::set` method"))
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Starting SyntheticService WASM server...");

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);

    let engine = Engine::new(&config)?;

    //Read path from arguments or use default
    let wasm_path = std::env::args().nth(1).unwrap_or_else(|| "./syntheticservicewasm.wasm".into());
    println!("Loading WASM component from: {}", wasm_path);
    let component = Component::from_file(&engine, wasm_path)
        .context("Failed to load WASM component")?;

    println!("WASM component loaded successfully");

    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

    let pre = ServicePre::new(linker.instantiate_pre(&component)?)?;
    let server = Server { pre };

    let port = std::env::var("PORT").unwrap_or_else(|_| "50055".to_string());
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

            if let Err(_err) = Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {}
        });
    }
}
