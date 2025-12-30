FROM rust:1.83 AS builder

WORKDIR /usr/src/app

# Install protobuf compiler
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

# Copy proto files
COPY protos ./protos

# Copy shippingservice files
COPY src/shippingservice/Cargo.toml src/shippingservice/Cargo.lock ./
COPY src/shippingservice/build.rs ./build.rs
COPY src/shippingservice/src ./src

# Build only the server binary in release mode
RUN cargo build --release --bin shipping-server

# Runtime stage
FROM debian:bookworm-slim

# Install necessary runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy the server binary from builder
COPY --from=builder /usr/src/app/target/release/shipping-server /usr/local/bin/shipping-server

# Expose the gRPC port
EXPOSE 50051

# Run the server
CMD ["shipping-server"]