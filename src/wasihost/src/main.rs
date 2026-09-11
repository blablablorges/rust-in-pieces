//! wasihost: serve one wasi:http component (0.3 handler, or 0.2 proxy) over HTTP/1.1 and h2c,
//! with a pooled outbound client that speaks h2 prior-knowledge to listed authorities.
//!
//!   wasihost <component.wasm|component.cwasm>
//!   PORT, WASIHOST_H2C_AUTHORITIES=host:port,..., WASIHOST_INSTANCE_REUSE (0 = unbounded),
//!   WASIHOST_INSTANCE_CONCURRENCY, WASIHOST_POOLING=0 to disable the pooling allocator.
use anyhow::{Context as _, Result};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use wasmtime::component::{Component, GuestTaskId, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreContextMut};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::handler::{
    HandlerState, Instance, ProxyHandler, ProxyPre, ShouldAccept, WorkerExpiration, WorkerState,
    WorkerStatus,
};
use wasmtime_wasi_http::{
    RequestOptions, WasiBody, WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
};

type Outbound = Client<HttpConnector, WasiBody>;
type HookFuture<T> = Box<dyn Future<Output = T> + Send>;
type IoResult = Result<(), wasmtime_wasi_http::Error>;

#[derive(Clone)]
struct Clients {
    h1: Outbound,
    h2: Outbound,
    h2c: Arc<Vec<String>>,
}

impl Clients {
    fn new(h2c: Vec<String>) -> Self {
        let connector = || {
            let mut c = HttpConnector::new();
            c.set_nodelay(true);
            c
        };
        let h1 = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(90))
            .build(connector());
        let h2 = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_only(true)
            .build(connector());
        Self { h1, h2, h2c: Arc::new(h2c) }
    }
}

struct Hooks(Clients);

impl WasiHttpHooks for Hooks {
    fn default_scheme(&mut self) -> Option<http::uri::Scheme> {
        Some(http::uri::Scheme::HTTP)
    }

    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: HookFuture<IoResult>,
    ) -> HookFuture<Result<(http::Response<WasiBody>, HookFuture<IoResult>), wasmtime_wasi_http::Error>> {
        let authority = request.uri().authority().map(|a| a.to_string()).unwrap_or_default();
        let client = if self.0.h2c.iter().any(|a| *a == authority) { self.0.h2.clone() } else { self.0.h1.clone() };
        Box::new(async move {
            let resp = client.request(request).await.map_err(|e| {
                eprintln!("outbound {authority}: {e}");
                if e.is_connect() {
                    wasmtime_wasi_http::Error::ConnectionRefused
                } else {
                    wasmtime_wasi_http::Error::HttpProtocolError
                }
            })?;
            let resp = resp.map(|b| b.map_err(wasmtime_wasi_http::Error::from).boxed_unsync());
            let done: HookFuture<IoResult> = Box::new(async { Ok(()) });
            Ok((resp, done))
        })
    }
}

struct Host {
    table: ResourceTable,
    ctx: WasiCtx,
    http: WasiHttpCtx,
    hooks: Hooks,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.ctx, table: &mut self.table }
    }
}

impl WasiHttpView for Host {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView { ctx: &mut self.http, table: &mut self.table, hooks: &mut self.hooks }
    }
}

fn new_host(instance_id: u64, clients: &Clients) -> Host {
    let mut b = WasiCtxBuilder::new();
    b.inherit_stdio().inherit_env().inherit_network().allow_ip_name_lookup(true);
    b.env("INSTANCE_ID", instance_id.to_string());
    Host { table: ResourceTable::new(), ctx: b.build(), http: WasiHttpCtx::new(), hooks: Hooks(clients.clone()) }
}

struct Expiration {
    idle: Duration,
    request: Duration,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

impl WorkerExpiration for Expiration {
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>, status: WorkerStatus, start: Instant) -> Poll<()> {
        let me = self.get_mut();
        let timeout = match status {
            WorkerStatus::Idle => me.idle,
            WorkerStatus::Requests | WorkerStatus::PostReturn => me.request,
        };
        match start.checked_add(timeout) {
            Some(deadline) => {
                let deadline = deadline.into();
                if deadline != me.sleep.deadline() {
                    me.sleep.as_mut().reset(deadline);
                }
                me.sleep.as_mut().poll(cx)
            }
            None => Poll::Pending,
        }
    }
}

struct Worker {
    reuse: usize,
    concurrency: usize,
    request_timeout: Duration,
}

impl WorkerState for Worker {
    type StoreData = Host;
    type RequestData = u64;

    fn should_accept_request(&self, concurrent: usize, total: usize) -> ShouldAccept {
        if self.reuse != 0 && total >= self.reuse {
            ShouldAccept::Never
        } else if concurrent >= self.concurrency {
            ShouldAccept::No
        } else {
            ShouldAccept::Yes
        }
    }

    fn on_request_start(
        &self,
        _store: StoreContextMut<'_, Host>,
        _request_id: u64,
        _task: GuestTaskId,
    ) -> Pin<Box<dyn Future<Output = ()> + 'static + Send + Sync>> {
        Box::pin(tokio::time::sleep(self.request_timeout))
    }

    fn drop(&self, store: Store<Host>, result: Result<(), wasmtime::Error>) {
        if let Err(e) = result {
            eprintln!("worker failed: {e:?}");
        }
        drop(store);
    }
}

struct State {
    engine: Engine,
    pre: ProxyPre<Host>,
    clients: Clients,
    reuse: usize,
    concurrency: usize,
    request_timeout: Duration,
    next_instance: AtomicU64,
    next_request: AtomicU64,
}

impl HandlerState for State {
    type StoreData = Host;
    type WorkerExpiration = Expiration;
    type WorkerState = Worker;

    async fn instantiate(&self) -> wasmtime::Result<Instance<Host, Expiration, Worker>> {
        let id = self.next_instance.fetch_add(1, Relaxed);
        let mut store = Store::new(&self.engine, new_host(id, &self.clients));
        let proxy = self.pre.instantiate_async(&mut store).await?;
        Ok(Instance {
            store,
            proxy,
            view: WasiHttpView::http,
            expiration: Expiration {
                idle: Duration::from_secs(3600),
                request: self.request_timeout,
                sleep: Box::pin(tokio::time::sleep(Duration::MAX)),
            },
            state: Worker { reuse: self.reuse, concurrency: self.concurrency, request_timeout: self.request_timeout },
        })
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn engine() -> Result<Engine> {
    let mut cfg = Config::new();
    cfg.wasm_component_model(true);
    cfg.wasm_component_model_async(true);
    cfg.async_support(true);
    if std::env::var("WASIHOST_POOLING").map(|v| v != "0").unwrap_or(true) {
        let pooling = wasmtime::PoolingAllocationConfig::default();
        cfg.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pooling));
        if let Ok(e) = Engine::new(&cfg) {
            return Ok(e);
        }
        eprintln!("pooling allocator unavailable, falling back to on-demand allocation");
        cfg.allocation_strategy(wasmtime::InstanceAllocationStrategy::OnDemand);
    }
    Ok(Engine::new(&cfg)?)
}

async fn handle(
    handler: ProxyHandler<State>,
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<WasiBody>, std::convert::Infallible> {
    let id = handler.state().next_request.fetch_add(1, Relaxed);
    let req = req.map(|b| b.map_err(wasmtime_wasi_http::Error::from).boxed_unsync());
    match handler.handle(id, req).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            eprintln!("request {id}: {e:?}");
            let body = http_body_util::Full::new(Bytes::from_static(b"wasihost: guest error"))
                .map_err(wasmtime_wasi_http::Error::from)
                .boxed_unsync();
            Ok(hyper::Response::builder().status(500).body(body).unwrap())
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("usage: wasihost <component>")?;
    let engine = engine()?;
    let component = if path.ends_with(".cwasm") {
        unsafe { Component::deserialize_file(&engine, &path)? }
    } else {
        Component::from_file(&engine, &path)?
    };
    let mut linker = Linker::<Host>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    let instance = linker.instantiate_pre(&component)?;
    let pre = match wasmtime_wasi_http::p3::bindings::ServicePre::new(instance.clone()) {
        Ok(p) => {
            eprintln!("component exports wasi:http/handler@0.3");
            ProxyPre::P3(p)
        }
        Err(_) => {
            eprintln!("component exports wasi:http/incoming-handler@0.2");
            ProxyPre::P2(wasmtime_wasi_http::p2::bindings::ProxyPre::new(instance)?)
        }
    };
    let h2c: Vec<String> = std::env::var("WASIHOST_H2C_AUTHORITIES")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    let state = State {
        engine,
        pre,
        clients: Clients::new(h2c),
        reuse: env_usize("WASIHOST_INSTANCE_REUSE", 0),
        concurrency: env_usize("WASIHOST_INSTANCE_CONCURRENCY", 16),
        request_timeout: Duration::from_secs(env_usize("WASIHOST_REQUEST_TIMEOUT_S", 30) as u64),
        next_instance: AtomicU64::new(0),
        next_request: AtomicU64::new(0),
    };
    let handler = ProxyHandler::new(state);
    let port: u16 = std::env::var("PORT").unwrap_or_else(|_| "50055".into()).parse()?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("wasihost serving {path} on :{port}");
    loop {
        let (stream, _) = listener.accept().await?;
        stream.set_nodelay(true)?;
        let handler = handler.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req| handle(handler.clone(), req));
            if let Err(e) = AutoBuilder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), svc)
                .await
            {
                eprintln!("connection: {e}");
            }
        });
    }
}
