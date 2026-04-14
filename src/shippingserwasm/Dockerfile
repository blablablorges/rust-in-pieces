FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

COPY protos /build/protos
COPY src/shippingserwasm/ /build/src/shippingserwasm/

WORKDIR /build/src/shippingserwasm

RUN cargo build --bin server --release

FROM mirror.gcr.io/debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/src/shippingserwasm/target/release/server .

ENV PORT="50051"
EXPOSE 50051

ENTRYPOINT ["./server"]
