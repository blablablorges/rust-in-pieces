#!/usr/bin/env bash
# skaffold custom builder for a wasm component (WP-H0).
#
#   hack/wasm-image.sh <crate-dir> <module.wasm> [cargo build args...]
#
# skaffold sets IMAGE (full ref incl. tag), PUSH_IMAGE and BUILD_CONTEXT. The
# component is compiled for wasm32-wasip2, wrapped by oci-tar-builder into an OCI
# layout whose layer carries the wasm media type (what lets the shim precompile
# and cache it — a plain rootfs tar would be JIT-compiled on every pod start),
# imported into a containerd and pushed to the registry named in IMAGE. From
# there kubelet pulls it exactly like the native image next to it.
#
# Where containerd is:
#   KIND_NODE=<container>  -> docker cp + docker exec <node> ctr ...   (kind)
#   otherwise              -> ${CTR:-sudo ctr} on this host             (cp1)
# Both honour /etc/containerd/certs.d, so the registry host in IMAGE resolves the
# same way for the push as it does for kubelet's pull.
set -euo pipefail

CRATE_DIR="$1"; MODULE="$2"; shift 2
: "${IMAGE:?skaffold sets IMAGE}"; : "${BUILD_CONTEXT:=$(pwd)}"
[[ "${PUSH_IMAGE:-true}" == "true" ]] || { echo "PUSH_IMAGE=false is unsupported: wasm images only exist in a registry (set build.local.push: true)" >&2; exit 1; }

# IMAGE = <repo>/<name>:<tag>; oci-tar-builder wants the three parts.
TAG="${IMAGE##*:}"; REF="${IMAGE%:*}"
NAME="${REF##*/}"; REPO="${REF%/*}"

(cd "${BUILD_CONTEXT}/${CRATE_DIR}" && cargo build --release --target wasm32-wasip2 "$@")
WASM="${BUILD_CONTEXT}/${CRATE_DIR}/target/wasm32-wasip2/release/${MODULE}"
[[ -f "$WASM" ]] || { echo "no ${WASM} after cargo build" >&2; exit 1; }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
TAR="${TMP}/${NAME}.tar"
oci-tar-builder --name "$NAME" --repo "$REPO" --tag "$TAG" --module "$WASM" -o "$TAR"

if [[ -n "${KIND_NODE:-}" ]]; then
  # /var/tmp, not /tmp: kind mounts a tmpfs over /tmp after docker cp would write.
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
