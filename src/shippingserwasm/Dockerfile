# ---------- Stage 1: Build the WASM component and native server ----------
FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

# Install the wasm32-wasip2 target
RUN rustup target add wasm32-wasip2

# Copy proto definitions
COPY protos /build/protos

# Copy the service source
COPY src/shippingserwasm/ /build/src/shippingserwasm/

# Copy the WIT submodule
COPY wit/ /build/wit/

WORKDIR /build/src/shippingserwasm

# Build the WASM component
RUN cargo build --lib --target wasm32-wasip2 --release && \
    cp target/wasm32-wasip2/release/shippingserwasm.wasm .

# Build the native Wasmtime server binary
RUN cargo build --bin serve --release

# ---------- Stage 2: Minimal runtime image ----------
FROM mirror.gcr.io/debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the WASM component and the native server binary
COPY --from=builder /build/src/shippingserwasm/shippingserwasm.wasm .
COPY --from=builder /build/src/shippingserwasm/target/release/serve .

ENV PORT="50051"
EXPOSE 50051

ENTRYPOINT ["./serve"]
