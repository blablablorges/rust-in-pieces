# Copyright 2026

FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

COPY protos /build/protos
COPY src/cartserwasm/ /build/src/cartserwasm/

WORKDIR /build/src/cartserwasm

RUN cargo build --bin server --release

FROM mirror.gcr.io/debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/src/cartserwasm/target/release/server .

ENV PORT="7070"
ENV REDIS_ADDR="redis-cart:6379"
EXPOSE 7070

ENTRYPOINT ["./server"]
