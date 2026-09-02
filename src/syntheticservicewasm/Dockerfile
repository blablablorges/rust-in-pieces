FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.86-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

COPY protos /build/protos
COPY src/lib /build/src/lib
COPY src/syntheticservicewasm/ /build/src/syntheticservicewasm/

WORKDIR /build/src/syntheticservicewasm

RUN cargo build --bin server --release

FROM mirror.gcr.io/debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/src/syntheticservicewasm/target/release/server .

ENV PORT="50055"
EXPOSE 50055

ENTRYPOINT ["./server"]
