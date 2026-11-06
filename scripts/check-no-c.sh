#!/usr/bin/env bash
# No-C rule: fail on any package named *-sys or with a `links` field in the
# normal + build dependency graph of the shipped crates. Allowlist is empty.
# (cargo-deny wildcard bans are a silent no-op, so this walks `cargo metadata`.)
# Dev-dependencies (criterion etc.) are not part of what ships and are skipped.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --locked --all-features | python3 -I -c '
import json, sys
m = json.load(sys.stdin)
pk = {p["id"]: p for p in m["packages"]}
nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
roots = [i for i, p in pk.items() if p["name"] in ("fontisan", "fontisan-ffi") and i in m["workspace_members"]]
if len(roots) != 2:
    sys.exit("check-no-c: expected workspace members fontisan and fontisan-ffi, got %d" % len(roots))
seen, stack = set(), list(roots)
while stack:
    i = stack.pop()
    if i in seen:
        continue
    seen.add(i)
    for d in nodes[i]["deps"]:
        if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
            stack.append(d["pkg"])
bad = sorted("%s %s (%s)" % (pk[i]["name"], pk[i]["version"], "links=" + pk[i]["links"] if pk[i].get("links") else "name *-sys")
             for i in seen if pk[i]["name"].endswith("-sys") or pk[i].get("links"))
print("check-no-c: %d packages in the normal+build graph" % len(seen))
if bad:
    print("check-no-c: FAIL\n  " + "\n  ".join(bad))
    sys.exit(1)
print("check-no-c: ok")
'
