#!/usr/bin/env python3
"""Fair-deployment check: every overlay must give the same service the same
resources and probes whatever runtime it runs under, and the same replica
count within an overlay family (boutique vs synthetic, with/without autoscaling).

    python3 kustomize/test_symmetry.py [kustomize-dir]

Renders each overlay with `kubectl kustomize` and asserts. Exit 1 on the first
asymmetry. This is the WP-A2 exit criterion and half of WP-G0.
"""
import subprocess, sys, pathlib, yaml

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).parent)
overlays = sorted(p for p in (root / "overlays").iterdir() if (p / "kustomization.yaml").exists())
assert overlays, f"no overlays under {root}"

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
