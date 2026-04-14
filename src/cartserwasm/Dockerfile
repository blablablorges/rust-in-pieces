# Copyright 2026
#
# WASM + native Rust container for cartserwasm (Redis-backed via TCP sockets).

FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

# Install the wasm32-wasip2 target
RUN rustup target add wasm32-wasip2

COPY protos /build/protos
COPY src/cartserwasm/ /build/src/cartserwasm/
COPY wit/ /build/wit/

WORKDIR /build/src/cartserwasm

# Build the WASM component
RUN cargo build --lib --target wasm32-wasip2 --release && \
    cp target/wasm32-wasip2/release/cartserwasm.wasm .

# Build the native Wasmtime server binary
RUN cargo build --bin serve --release

FROM mirror.gcr.io/debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the WASM component and the native server binary
COPY --from=builder /build/src/cartserwasm/cartserwasm.wasm .
COPY --from=builder /build/src/cartserwasm/target/release/serve .

ENV PORT="7070"
ENV REDIS_ADDR="redis-cart:6379"
EXPOSE 7070

ENTRYPOINT ["./serve"]
