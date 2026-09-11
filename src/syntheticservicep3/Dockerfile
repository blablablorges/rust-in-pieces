FROM --platform=$BUILDPLATFORM mirror.gcr.io/rust:1.98-bookworm AS build
RUN apt-get update && apt-get install -y protobuf-compiler xz-utils && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-wasip2
ARG WASMTIME_VERSION=48.0.2
ARG TARGETARCH
RUN case "$TARGETARCH" in amd64) A=x86_64 ;; arm64) A=aarch64 ;; *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; esac \
 && curl -fsSL "https://github.com/bytecodealliance/wasmtime/releases/download/v${WASMTIME_VERSION}/wasmtime-v${WASMTIME_VERSION}-${A}-linux.tar.xz" | tar -xJ -C /usr/local/bin --strip-components=1 "wasmtime-v${WASMTIME_VERSION}-${A}-linux/wasmtime"
COPY protos /build/protos
COPY src/wasihost /build/src/wasihost
COPY src/syntheticservicep3 /build/src/syntheticservicep3
WORKDIR /build/src/syntheticservicep3
RUN cargo build --release --target wasm32-wasip2 \
 && case "$TARGETARCH" in amd64) T=x86_64-unknown-linux-gnu ;; arm64) T=aarch64-unknown-linux-gnu ;; esac \
 && wasmtime compile --target "$T" -o /synthetic.cwasm target/wasm32-wasip2/release/syntheticservicep3.wasm
WORKDIR /build/src/wasihost
RUN cargo build --release && cp target/release/wasihost /wasihost

FROM mirror.gcr.io/debian:bookworm-slim
COPY --from=build /wasihost /app/wasihost
COPY --from=build /synthetic.cwasm /app/synthetic.cwasm
USER 65534
ENV PORT="50055"
EXPOSE 50055
ENTRYPOINT ["/app/wasihost", "/app/synthetic.cwasm"]
