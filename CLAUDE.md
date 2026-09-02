# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Fork of Google's [Online Boutique](https://github.com/GoogleCloudPlatform/microservices-demo) with several services rewritten in Rust and compiled to WebAssembly (WASI P2). Research project comparing Wasm microservice execution against native containers on Kubernetes.

## Build commands

### Wasm services (compile to `.wasm` component)

Each `*serwasm` crate builds two artifacts:

1. **Wasm component** (guest — runs inside wasmtime):
```sh
cd src/<SERVICE>serwasm
cargo build --lib --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/<SERVICE>serwasm.wasm .
```

2. **Native host server** (serve.rs — runs wasmtime, exposes gRPC):
```sh
cargo build --bin serve --release
```

3. **Native standalone server** (server.rs — no wasmtime, direct gRPC):
```sh
cargo build --bin server --release
```

Required toolchain: `rustup target add wasm32-wasip2`

### Native Rust shipping service (non-Wasm baseline)

```sh
cd src/shippingservice
cargo build
```

### Full cluster deploy (Skaffold + Minikube)

```sh
minikube start --cpus=4 --memory 4096 --disk-size 32g
skaffold run        # build + deploy all services
skaffold dev        # rebuild on code change
skaffold delete     # cleanup
kubectl port-forward deployment/frontend 8080:8080
```

### Test a Rust service

```sh
# shipping gRPC client against a running server
cd src/shippingservice
cargo run --bin shipping-client
```

## Architecture

### Two-tier design in Wasm services

Each `*serwasm` crate contains:

- `src/main.rs` — **Wasm guest**: implements gRPC service logic using `wasi_grpc_server::grpc_component` macro. Compiled to `.wasm` component targeting `wasm32-wasip2`. Has no system dependencies (uses WASI sockets/HTTP for Redis and outbound calls).
- `bin/serve.rs` — **Native host**: loads and runs the `.wasm` component via wasmtime, exposes gRPC over TCP. Uses `wasmtime`, `wasmtime-wasi`, `wasmtime-wasi-http`.
- `bin/server.rs` — **Native standalone**: same gRPC interface, standard Rust deps (tokio, redis crate). Used for the containerized baseline and Docker images.
- `src/lib/wasi-grpc-server/` (one copy, shared by all `*serwasm` crates via path dependency) — proc-macro crate that implements the `#[grpc_component]` attribute, wiring tonic gRPC dispatch into the WASI HTTP incoming-handler interface.

The Cargo.toml uses `[target.'cfg(not(target_family = "wasm"))'.dependencies]` to gate native-only deps (wasmtime, tokio, redis) out of the Wasm build.

### WIT world (`./wit/`)

Custom WIT world (`service`) extends `wasi:http/proxy@0.2.2` with:
- `wasi:sockets/imports@0.2.2` — raw TCP for Redis (cartserwasm, syntheticservicewasm)
- `wasi:cli/environment@0.2.2` — env var access

Both host (`serve.rs`, wasmtime bindgen) and guest (`main.rs`, wit_bindgen) generate bindings from `../../wit`.

### Services

| Service | Language | Notes |
|---|---|---|
| `shippingserwasm` | Rust/Wasm | Shipping cost/tracking — simplest Wasm service, no external deps |
| `recommendationserwasm` | Rust/Wasm | Product recommendations — outbound gRPC to productcatalog via WASI HTTP |
| `cartserwasm` | Rust/Wasm | Shopping cart — Redis via WASI sockets |
| `syntheticservicewasm` | Rust/Wasm | Benchmark-only synthetic service — exercises compute/network/data workloads |
| `shippingservice` | Rust (native) | Original Rust rewrite of the Go shipping service; also provides `shipping-client` test binary |
| All others | Go/Python/Node/Java/C# | Upstream Google microservices, unchanged |

### Kustomize overlays (`./kustomize/overlays/`)

| Overlay | Description |
|---|---|
| `baseline` | All services as containers |
| `wasi-vanilla` | shippingserwasm deployed via wasmtime host |
| `wasi-grpc` | shipping + recommendation as Wasm |
| `wasi-tcp` | shipping + recommendation + cart as Wasm |
| `wasi-all` | All Wasm services |
| `synthetic-baseline` | Synthetic service as container |
| `synthetic-wasm` | Synthetic service as Wasm |

### Proto definitions

All gRPC service definitions in `./protos/demo.proto`. Each Rust crate's `build.rs` calls `tonic-build` to generate bindings from this shared proto.

## Key dependencies

- **wasmtime 27.0** with `component-model` + `async` features
- **tonic 0.13.1** — gRPC, used in both Wasm guest (codegen only, no transport) and native host
- **wasi-hyperium 0.3.0** — bridges WASI HTTP types to Hyper for use inside Wasm
- **wasi 0.13.1** — raw WASI bindings for socket calls
