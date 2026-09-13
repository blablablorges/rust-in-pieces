#!/usr/bin/env bash
# skaffold custom builder for the TinyGo synthetic component: hack/wasm-image.sh with the
# cargo step replaced by `tinygo build` (wasip2 target, wasi:http proxy world). Same
# oci-tar-builder / ctr import / push path, so the shim precompiles and caches it.
#
#   hack/tinygo-image.sh <crate-dir> <module.wasm>
#
# Needs tinygo and wasm-tools on PATH. KIND_NODE / CTR / BUILD_CONTEXT as in wasm-image.sh.
set -euo pipefail

CRATE_DIR="$1"; MODULE="$2"; shift 2
: "${IMAGE:?skaffold sets IMAGE}"; : "${BUILD_CONTEXT:=$(pwd)}"
[[ "${PUSH_IMAGE:-true}" == "true" ]] || { echo "PUSH_IMAGE=false is unsupported: wasm images only exist in a registry (set build.local.push: true)" >&2; exit 1; }

TAG="${IMAGE##*:}"; REF="${IMAGE%:*}"
NAME="${REF##*/}"; REPO="${REF%/*}"

(cd "${BUILD_CONTEXT}/${CRATE_DIR}" && tinygo build -opt=2 -target ./wasip2-synthetic.json -o "${MODULE}" .)
WASM="${BUILD_CONTEXT}/${CRATE_DIR}/${MODULE}"
[[ -f "$WASM" ]] || { echo "no ${WASM} after tinygo build" >&2; exit 1; }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
TAR="${TMP}/${NAME}.tar"
oci-tar-builder --name "$NAME" --repo "$REPO" --tag "$TAG" --module "$WASM" -o "$TAR"

if [[ -n "${KIND_NODE:-}" ]]; then
  docker cp "$TAR" "${KIND_NODE}:/var/tmp/${NAME}.tar"
  CTR="docker exec ${KIND_NODE} ctr"; TAR="/var/tmp/${NAME}.tar"
else
  CTR="${CTR:-sudo ctr}"
fi
$CTR -n k8s.io images import --all-platforms "$TAR"
$CTR -n k8s.io images push --hosts-dir /etc/containerd/certs.d "$IMAGE"
$CTR -n k8s.io images rm "$IMAGE" >/dev/null
[[ -n "${KIND_NODE:-}" ]] && docker exec "${KIND_NODE}" rm -f "$TAR"
echo "pushed ${IMAGE}"
