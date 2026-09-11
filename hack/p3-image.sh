#!/usr/bin/env bash
# skaffold custom builder for the p3 arm, offline where it can be:
#   component built on the host (target-independent), wasihost built for the node's arch in a
#   rust container with the host's cargo registry mounted, both packaged by Dockerfile.prebuilt.
set -euo pipefail
: "${IMAGE:?skaffold sets IMAGE}"; : "${BUILD_CONTEXT:=$(pwd)}"
cd "$BUILD_CONTEXT"
case "$(uname -m)" in arm64|aarch64) PLATFORM=linux/arm64; ARCH=arm64 ;; *) PLATFORM=linux/amd64; ARCH=amd64 ;; esac

(cd src/syntheticservicep3 && cargo build --release --target wasm32-wasip2 -q)
WASM=src/syntheticservicep3/target/wasm32-wasip2/release/syntheticservicep3.wasm

HOSTBIN="target/wasihost-linux-${ARCH}/wasihost"
if [[ ! -x "$HOSTBIN" ]] || [[ -n "$(find src/wasihost/src src/wasihost/Cargo.toml -newer "$HOSTBIN" 2>/dev/null)" ]]; then
  docker run --rm --platform "$PLATFORM" -v "$PWD":/work -w /work/src/wasihost \
    -v "$HOME/.cargo/registry":/usr/local/cargo/registry -v "wasihost-target-${ARCH}":/target \
    -e CARGO_TARGET_DIR=/target -e CARGO_NET_OFFLINE=true rust:1.98-bookworm \
    bash -c "cargo build --release --offline -q && mkdir -p /work/target/wasihost-linux-${ARCH} && cp /target/release/wasihost /work/target/wasihost-linux-${ARCH}/"
fi

STAGE=target/p3-image; mkdir -p "$STAGE"
cp "$WASM" "$STAGE/synthetic.wasm"; cp "$HOSTBIN" "$STAGE/wasihost"
docker build --platform "$PLATFORM" -q -f src/syntheticservicep3/Dockerfile.prebuilt -t "$IMAGE" "$STAGE" >/dev/null
[[ "${PUSH_IMAGE:-true}" == "true" ]] && docker push -q "$IMAGE" >/dev/null
echo "built ${IMAGE}"
