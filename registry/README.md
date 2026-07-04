# Claw package registry (format)

Claw's registry is **content-addressed**, mirroring the code-as-database:
a package is a set of definitions identified by the hash of their content,
so a version is immutable and a name is just a pointer. This is the same
model that makes rename O(1) and makes "does this symbol exist?" a
decidable question — extended to distribution.

## Index format

`index.json` maps package names to versions, each version to the content
hashes of its definitions:

```json
{
  "packages": {
    "std/nat": {
      "1.0.0": {
        "defs": {
          "Nat.add": "a1f3…",
          "Nat.mul": "0c9e…"
        },
        "requires": []
      }
    }
  }
}
```

## Why content-addressed

- **Immutable versions** — a hash can't change under you; no left-pad.
- **Dedup by construction** — two packages sharing a definition share the
  hash; it's stored once.
- **Verifiable** — a client checks the content against the hash; no trust
  in the registry's integrity needed.
- **Agent-native** — the same `candidates(type)` query that constrains
  generation locally works against the registry: "what real symbols of
  this type can I depend on?"

## Interop (the adoption path)

A Claw package can declare a Rust FFI dependency (see `claw-emit-rust`), so
publishing to this registry does not cut you off from crates.io — the
registry indexes Claw definitions, and `--emit=rust` lets the outside world
consume them. Inherit the ecosystem instead of rebuilding it.

## Status

Format spec only. The serving implementation (a static index + content
store, then a queryable service) is future work — but the format is fixed
by the same content-addressing the CDB already implements.
