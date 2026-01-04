# shippingservice wasm

This is the wasm-ready version of the shipping-service.

Prerequisites:

* rustup: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
* wasm32-wasip2: `rustup target add wasm32-wasip2`

## deployment

`src/main.rs` contains the rust source code

Compile it to a webassembly component via
```
cargo build --lib --target wasm32-wasip2 --release && \
cp target/wasm32-wasip2/release/shippingserwasm.wasm .
```
Once compiled, build and run a server that and executes a WebAssembly component using Wasmtime's WASI runtime
```
cargo build --bin serve
```
```
cargo run --bin serve
```
