# Writing a Wasm Guest for the Wasmtime Shim

How to implement a service as a WASI P2 Wasm **component** that runs under
`containerd-shim-wasmtime`. This describes the host contract only — the
interfaces the host exposes and the rules it follows. It makes no assumptions
about your language conventions, crate names, proto package, backend (Redis,
Kafka, a database, anything), or whether you also ship a native binary. If your
component honours the contract below, it runs.

Two outbound styles are covered:

- **gRPC / HTTP guest** — inbound over `wasi:http`; outbound HTTP/1.1 or h2c gRPC
  to other services, pooled by the host.
- **TCP guest** — outbound request/reply to a byte/line backend via the host's
  `pooled-tcp` interface, pooled and framed by the host.

A single component can use either, both, or neither (a pure-compute service just
implements the inbound handler).

---

## 1. The host contract

These are properties of the host, true for any guest:

- **One instance per inbound request.** The `wasi:http/proxy` reactor world
  instantiates a fresh component instance per request and drops it on
  completion. **Nothing in a guest global survives between requests** — not a
  connection, a client, nor a cache. State that must persist lives host-side.
- **The host owns connection reuse.** Pooling, keep-alive, liveness, and reply
  framing for outbound calls are the host's job, transparent to the guest. The
  guest issues a *logical* request; the host decides whether it rides a fresh or
  a reused connection.
- **No raw sockets in the guest.** Outbound goes through `wasi:http` (HTTP/1.1,
  h2c) or the host's `pooled-tcp` interface. The guest never calls
  `wasi:sockets`.
- **Inbound is HTTP-only.** `wasi:http/proxy` has no raw-TCP incoming handler.
  Every service is reached over HTTP; gRPC rides HTTP/2 over that.
- **Config arrives as environment variables.** The host injects them; the guest
  reads them with its env API (over `wasi:cli/environment`). Routing/pooling
  config is set on the *host*, not the guest (see §3, §4).

You target `wasm32-wasip2` and produce a component. Nothing else about your
project — build system, file layout, naming — is constrained by the host.

> The examples below are Rust because the reference services are Rust. The
> contract is language-agnostic: any toolchain that emits a `wasm32-wasip2`
> component implementing `wasi:http/incoming-handler` (and, for TCP, importing
> the `pooled-tcp` interface) works the same way.

---

## 2. Minimum viable guest (inbound)

A guest must export the `wasi:http` incoming handler. Everything else is
optional. The smallest useful service receives a request and replies — no
outbound calls.

Two ways to provide the handler:

1. **Raw `wasi:http`** — bind the `wasi:http/proxy` world (e.g. with
   `wit_bindgen::generate!` or `cargo component`), implement
   `incoming_handler::Guest::handle`, read the request, write the response.
   Fully generic; works for REST, gRPC, anything HTTP.

2. **A gRPC convenience layer** — if you want tonic-style gRPC without writing
   the HTTP plumbing, wrap the handler. The reference services use a small
   proc-macro (`wasi-grpc-server`) that wires tonic dispatch into the incoming
   handler; see §3. This is a convenience, not a requirement.

Keep transport code at the edges and put logic in a transport-agnostic module so
the same logic can be reused behind a different front end (e.g. a native server,
if your project ships one — the host neither knows nor cares).

---

## 3. gRPC / HTTP guest

### Inbound

If you use the `wasi-grpc-server` convenience macro: annotate a unit struct with
the generated server type and implement your tonic service trait. The macro
generates the `export!` and the `incoming_handler::Guest::handle` that runs
tonic dispatch.

```rust
use tonic::{Request, Response, Status};

// your proto, whatever its package/service names are
pub mod pb { tonic::include_proto!("your.package"); }
use pb::your_service_server::{YourService, YourServiceServer};
use pb::{YourRequest, YourReply};

#[wasi_grpc_server::grpc_component(YourServiceServer)]
struct Svc;

#[tonic::async_trait]
impl YourService for Svc {
    async fn your_method(
        &self,
        req: Request<YourRequest>,
    ) -> Result<Response<YourReply>, Status> {
        Ok(Response::new(handle(req.into_inner())))
    }
}
```

Generate tonic bindings with transport disabled — the guest uses codegen only,
not tonic's transport (no project-specific paths implied; point at your own
proto):

```rust
// build.rs
tonic_build::configure()
    .build_transport(false)
    .compile_protos(&["path/to/your.proto"], &["path/to/protos"])
    .unwrap();
```

If you'd rather not use the macro or tonic at all, implement
`incoming_handler::Guest::handle` directly and speak whatever HTTP you like.

> **gRPC error-trailers gotcha:** `Trailers-Only` gRPC error responses can lose
> their `grpc-status` trailers through the WASI HTTP layer, surfacing on the
> client as *"server closed the stream without sending trailers."* Where a
> degraded-but-valid response is acceptable, returning a valid message can be
> more robust than `Err(Status)`. This is a property of the HTTP layer, not your
> code.

### Outbound

There is no tonic transport in the guest, so make outbound calls over
`wasi:http`. For a unary gRPC call: frame the body as a length-prefixed gRPC
message, POST to `/<package>.<Service>/<Method>`, unframe the reply. For plain
REST, just send/receive the body. The host pools and reuses the underlying
connection.

```rust
// unary gRPC over wasi:http — sketch
fn call(addr: &str, path: &str, msg_bytes: &[u8]) -> Result<Vec<u8>, String> {
    // 5-byte gRPC frame: 1 compression flag + big-endian u32 length
    let mut frame = Vec::with_capacity(5 + msg_bytes.len());
    frame.push(0);
    frame.extend_from_slice(&(msg_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(msg_bytes);

    let headers = wasi::http::types::Fields::new();
    headers.append(&"content-type".into(), &b"application/grpc".to_vec())
        .map_err(|e| format!("{e:?}"))?;
    let req = wasi::http::types::OutgoingRequest::new(headers);
    req.set_method(&wasi::http::types::Method::Post).unwrap();
    req.set_scheme(Some(&wasi::http::types::Scheme::Http)).unwrap();
    req.set_authority(Some(addr)).unwrap();          // "host:port"
    req.set_path_with_query(Some(path)).unwrap();    // "/your.package.YourService/YourMethod"

    let body = req.body().unwrap();
    let stream = body.write().unwrap();
    let fut = wasi::http::outgoing_handler::handle(req, None).map_err(|e| format!("{e:?}"))?;
    stream.blocking_write_and_flush(&frame).map_err(|e| format!("{e:?}"))?;
    drop(stream);
    wasi::http::types::OutgoingBody::finish(body, None).map_err(|e| format!("{e:?}"))?;

    let p = fut.subscribe(); p.block();
    let resp = fut.get().ok_or("no response")?.map_err(|_| "future err")?.map_err(|e| format!("{e:?}"))?;
    if resp.status() != 200 { return Err(format!("status {}", resp.status())); }

    let input = resp.consume().unwrap().stream().unwrap();
    let mut buf = Vec::new();
    loop {
        match input.blocking_read(65536) {
            Ok(c) if c.is_empty() => break,
            Ok(c) => buf.extend_from_slice(&c),
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(e) => return Err(format!("{e:?}")),
        }
    }
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    Ok(buf[5..5 + len].to_vec())   // strip the gRPC frame header
}
```

Keep this plumbing behind a trait so your logic stays transport-free.

**Host pooling config.** The host routes each outbound *authority* to a protocol
pool. Set on the host (environment), keyed by `host:port` — substitute your own:

```
WASMTIME_HTTP_PROXY_OUTBOUND_RULES=some-service:50051=h2c,other-service:8080=http1
WASMTIME_HTTP_PROXY_OUTBOUND_DEFAULT=default        # default | http1 | h2c
```

`h2c` multiplexes gRPC over a pooled HTTP/2 connection; `http1` uses an HTTP/1.1
keep-alive pool; `default` is per-request. Pool tuning lives in `WASMTIME_POOL_*`
vars (see the host's `outbound.rs`).

---

## 4. TCP guest (pooled request/reply)

For a backend that speaks a raw byte/line protocol — Redis, a line-based cache,
a custom TCP service, anything with a clear reply boundary — do **not** open a
socket. Use the host's `pooled-tcp` interface: hand it `(upstream, request-bytes)`
and get back one framed reply. The host owns the pool, liveness probing, and
reply framing, so the connection is reused at a clean message boundary.

This interface is **protocol-agnostic**. Redis/RESP is one configured framing;
line-delimited is another. Your guest only encodes request bytes and parses reply
bytes — it does not know how the host frames the wire.

### Declare the import

`pooled-tcp` is not part of the inbound world, so bind it inline. Use the **same
`wit-bindgen` version as your `wasi` crate** so they share one `wit-bindgen-rt`
(otherwise you get a duplicate `cabi_realloc`):

```rust
mod pooled_tcp_bindings {
    wit_bindgen::generate!({
        inline: r#"
package hybrid:microservices;

interface pooled-tcp {
  variant tcp-error {
    unknown-upstream(string),
    connect-failed(string),
    timeout,
    protocol(string),
    io(string),
  }
  request: func(upstream: string, payload: list<u8>) -> result<list<u8>, tcp-error>;
}

world pooled-tcp-client { import pooled-tcp; }
"#,
    });
}
use pooled_tcp_bindings::hybrid::microservices::pooled_tcp;
```

This must match the host's `wit/pooled-tcp.wit` exactly. The host satisfies the
import via a manual linker definition; it is intentionally not in the bindgen
world.

### Issue a request

Encode your protocol's request, call `request`, parse the reply. The upstream is
just a `host:port` string the host has been told how to frame:

```rust
fn request(upstream: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
    // Host writes `payload`, reads exactly one framed reply, returns it.
    // Reuse / keepalive / liveness are host-side.
    pooled_tcp::request(upstream, payload).map_err(|e| format!("pooled-tcp: {e:?}"))
}
```

Build your protocol encode/decode on top (e.g. RESP encoding for Redis, or
newline-delimited for a line protocol). The framing the host applies to the
*reply* is configured host-side, not chosen by the guest.

### Host config for the upstream

The host must know each upstream and its reply framing — set on the host:

```
WASMTIME_RAWTCP_UPSTREAMS=cache:6379=resp,feed:9000=line   # framing: resp | line
```

An unregistered upstream returns `unknown-upstream`. Tuning lives in
`WASMTIME_RAWTCP_*` vars.

> **What request/reply does not cover:** server-push or stateful sessions —
> pub/sub, long-poll/streaming consumes, pipelining, or stateful command
> sequences that must pin one connection (`MULTI`/`SELECT`/`AUTH`-style). Pooled
> reuse at the message boundary may land the next request on a different
> connection. Single-shot commands are safe. Streaming protocols need a
> different interface.

---

## 5. Build & verify

Target the component and confirm its imports — names below are illustrative,
substitute your crate:

```sh
rustup target add wasm32-wasip2                       # once
cargo build --lib --target wasm32-wasip2 --release    # produces <crate>.wasm

# confirm the component imports what you expect (and nothing you dropped)
strings target/wasm32-wasip2/release/<crate>.wasm \
  | grep -E 'hybrid:microservices|wasi:'
```

A TCP guest should show `hybrid:microservices/pooled-tcp` and **no**
`wasi:sockets/tcp-create-socket`. A pure HTTP guest shows only `wasi:http` (and
`wasi:cli/environment` if it reads env).

How you run it — a native host harness for local dev, the shim under containerd,
a Kubernetes overlay — is up to your project. The host's only requirements are
the component target and the environment variables in §3/§4. The reference
project wires these through its own Kustomize overlays and Skaffold, but that is
project plumbing, not part of the contract.

---

## 6. Checklist

- [ ] Builds to `wasm32-wasip2` as a component.
- [ ] Exports `wasi:http/incoming-handler` (directly, or via a convenience layer).
- [ ] No raw sockets; no connection/client cached in a guest global (instance is per-request).
- [ ] Logic kept transport-free; WASI plumbing behind a trait/boundary.
- [ ] Outbound HTTP/gRPC: sent over `wasi:http`; authority routed to `h2c`/`http1` on the host.
- [ ] Outbound TCP: inline `pooled-tcp` bindings with `wit-bindgen` matching the `wasi` crate; upstream + framing registered via `WASMTIME_RAWTCP_UPSTREAMS`.
- [ ] `strings` on the `.wasm` shows the expected imports and nothing you removed.
