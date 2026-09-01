# Copyright 2026

# 1.87 is the floor, not a preference: the pooled-tcp migration in src/main.rs
# calls wit_bindgen::generate!, and wit-bindgen 0.51 declares rust-version
# 1.87.0. On 1.86 cargo refuses before compiling anything:
#   error: rustc 1.86.0 is not supported by the following packages:
#     wit-bindgen@0.51.0 requires rustc 1.87.0
#
# Only this service moves. shippingserwasm and recommendationserwasm still build
# on 1.85 and syntheticservicewasm on 1.86, and bumping those would change the
# compiler under services whose measurements are already published for no reason
# their dependencies ask for.
FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.87-bookworm AS builder

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
