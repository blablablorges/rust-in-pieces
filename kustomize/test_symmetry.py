#!/usr/bin/env python3
"""Fair-deployment check: every overlay must give the same service the same
resources, probes, nodeSelector and behaviour-relevant env whatever runtime it
runs under, and the same replica count within an overlay family (boutique vs
synthetic, with/without autoscaling).

Env keys that only wire the runtime (WASMTIME_*, listen ports) may differ between
a docker and a wasm member of a pair; anything else present on one side only, or
with a different value, fails — that is how a debug RUST_LOG on one side gets
caught (WP-A1) and how a missing WASMTIME_RAWTCP_UPSTREAMS would not (WP-G2's job).

    python3 kustomize/test_symmetry.py [kustomize-dir]

Renders each overlay with `kubectl kustomize` and asserts. Exit 1 on the first
asymmetry. This is the WP-A2 exit criterion and WP-G0's static half.
"""
import subprocess, sys, pathlib, yaml

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).parent)
overlays = sorted(p for p in (root / "overlays").iterdir() if (p / "kustomization.yaml").exists())
assert overlays, f"no overlays under {root}"

# Prefix (trailing _), suffix (leading _) or exact. Ports and addresses are wiring;
# whether the wiring actually reaches the backend is WP-G2's runtime check.
ENV_MAY_DIFFER = ("WASMTIME_", "_ADDR", "PORT", "SHIPPING_PORT")  # ponytail: extend when a port lands


def wiring(key):
    return any(key == a or (a.endswith("_") and key.startswith(a)) or (a.startswith("_") and key.endswith(a)) for a in ENV_MAY_DIFFER)


def comparable_env(container):
    return {e["name"]: e.get("value", "<valueFrom>") for e in container.get("env", []) if not wiring(e["name"])}


def family(name):  # ponytail: two families today; add a token here when a third appears
    return ("synthetic" if name.startswith("synthetic") else "boutique") + ("-hpa" if name.endswith("-with-autoscaling") else "")

seen = {}  # service -> (overlay, spec)
for o in overlays:
    out = subprocess.run(["kubectl", "kustomize", str(o)], check=True, capture_output=True, text=True).stdout
    for d in yaml.safe_load_all(out):
        if not d or d.get("kind") != "Deployment":
            continue
        name = d["metadata"]["name"]
        c = d["spec"]["template"]["spec"]["containers"][0]
        spec = {
            "resources": c.get("resources"),
            "readinessProbe": c.get("readinessProbe"),
            "livenessProbe": c.get("livenessProbe"),
            "nodeSelector": d["spec"]["template"]["spec"].get("nodeSelector"),
            "env": comparable_env(c),
        }
        replicas = d["spec"].get("replicas", 1)
        if name in seen:
            first_overlay, first = seen[name]
            assert spec == first, f"{name}: {o.name} != {first_overlay}\n  {spec}\n  {first}"
        else:
            seen[name] = (o.name, spec)
        key = (family(o.name), name)
        if key in seen:
            first_overlay, first = seen[key]
            assert replicas == first, f"{name} replicas: {o.name}={replicas} != {first_overlay}={first}"
        else:
            seen[key] = (o.name, replicas)

print(f"ok: {sum(isinstance(k, str) for k in seen)} deployments symmetric across {len(overlays)} overlays")
