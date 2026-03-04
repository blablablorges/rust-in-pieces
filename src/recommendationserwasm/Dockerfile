# Copyright 2024 Google LLC
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#      http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# ---------- Stage 1: Build the WASM component and native server ----------
FROM rust:1.82-bookworm AS builder

# Install the wasm32-wasip2 target
RUN rustup target add wasm32-wasip2

WORKDIR /build

# Copy proto definitions
COPY protos/ protos/

# Copy the service source
COPY src/recommendationserwasm/ src/recommendationserwasm/

WORKDIR /build/src/recommendationserwasm

# Build the WASM component
RUN cargo build --lib --target wasm32-wasip2 --release && \
    cp target/wasm32-wasip2/release/recommendationserwasm.wasm .

# Build the native Wasmtime server binary
RUN cargo build --bin serve --release

# ---------- Stage 2: Minimal runtime image ----------
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the WASM component and the native server binary
COPY --from=builder /build/src/recommendationserwasm/recommendationserwasm.wasm .
COPY --from=builder /build/src/recommendationserwasm/target/release/serve .

ENV PORT="8080"
EXPOSE 8080

ENTRYPOINT ["./serve"]
