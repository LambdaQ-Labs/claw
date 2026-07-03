# cdb/ — WS-B (Code-as-Database)

Content-addressed store of definitions. Source is a DB of hashed defs, not text files. Spec: [`../docs/p2-spec.md`](../docs/p2-spec.md) §1.

- Identity = `blake3(normalized_ast)`; names are mutable pointers → O(1) rename.
- MVP store: SQLite (`definitions`, `names`, `edges`).
- Query API: `symbols`, `type_at`, `candidates`, `callers`, `deps`, `search`, `def`, `render`.
- Edit API: `put`, `bind`, `edit`, `remove`.

`candidates(type, scope)` is the load-bearing call — it's what makes API hallucination structurally impossible and what the constraint server (WS-C) masks against.
