# Registry & playground: what the incumbents have, what Claw builds

Research pass over crates.io, npmjs.com, pkg.go.dev, play.rust-lang.org /
go.dev/play — feature inventory, then the Claw plan with the advantages a
content-addressed, machine-verified language makes possible.

## What the incumbents offer

### Package registries

| feature | crates.io (Rust) | npm (Node) | pkg.go.dev (Go) |
|---|---|---|---|
| Search | name + keywords | name + quality/popularity/maintenance ranking | name, SYMBOL search (`io.Reader`), `#filter` |
| Package page | README, versions, deps, **reverse deps**, download graph | README, weekly downloads, versions/dist-tags, dependents | auto-generated API docs from source, imports/imported-by |
| Install UX | `cargo add x` snippet | `npm i x` snippet | `go get` — no publish step at all (indexes git via module proxy) |
| Docs | docs.rs auto-builds every crate | manual README | **docs generated from code** — the killer feature |
| Trust | ownership, yank, semver | provenance attestations (sigstore), 2FA, audit | license detection, vuln DB integration |
| Versioning | semver + yank | semver + dist-tags | module versions from git tags |
| API | full REST API | full API | full API |
| Badges | shields.io | shields.io | official badge generator |

### Playgrounds

| feature | play.rust-lang.org | go.dev/play |
|---|---|---|
| Run real compiler server-side | ✅ stable/beta/nightly | ✅ (sandboxed, time-frozen) |
| Modes | run / build / test / ASM | run / tests |
| Tools | rustfmt, clippy, miri | gofmt |
| Share | permalink (gist-backed) | permalink |
| Examples | preloaded menu | preloaded menu |
| Cost to run | real servers + sandboxing | real servers + sandboxing |

## The Claw plan

### Registry — "the registry where every package is machine-verified"

We already have: `claw publish` / `claw add`, the registry service
(axum + Postgres + content-addressed blobs) **hosted at
registry.clawlang.dev** (the CLI default), and a package format. What
makes ours different is not parity — it's what content-addressing + the
grading pipeline enable that npm/crates structurally cannot do:

1. **Verified-on-publish badge.** Publishing runs the gate: compiles,
   contracts execute, effect rows checked. A package page says
   "121 defs · all typechecked · 14 contracts hold" — not "trust me".
2. **Type search across the whole ecosystem.** `candidates("Str -> Str")`
   over every published package — pkg.go.dev's symbol search, but
   type-directed. No other registry can answer "what, anywhere, fits here?"
3. **Docs from the CDB, free.** Like pkg.go.dev: every def renders its
   signature + doc + source. No docs.rs build farm needed — the package IS
   the database.
4. **No typosquatting economics.** Content hashes are identity; names are
   labels over hashes. `claw add foo` shows the hash + verification state.
5. **AI-consumable by design.** The registry speaks the same protocol as
   the local CDB — an agent's MCP `claw_candidates` can search the
   ecosystem, not just the project. That's the npm-for-agents story.

Build order (infra ≈ one small VPS or free-tier fly/CF):
- ~~v0: host the existing service~~ **done** — live at
  registry.clawlang.dev, with the MCP-compat invariant shipped: publish
  requires parseable defs (name/type/effects/doc, served at
  `GET /defs/:name/:version`), and `claw add` ingests them into the
  project CDB (point 5 above, delivered)
- v1: web UI — search, package page (defs, types, docs, verify badge),
  install snippet, versions
- v2: ecosystem type-search + MCP endpoint + provenance (sig over hash)

### Playground — real engine, zero servers

Rust/Go playgrounds cost real server fleets. We can skip that: claw-core
(parser, type unifier, interpreter, renderer) and claw-constraint are pure
Rust — they compile to **WebAssembly** as-is. The playground then runs the
ACTUAL engine in the browser:

- type-checks signatures with the real unifier
- evaluates programs with the real step-bounded interpreter
- shows the real decode grammar for the current scope
- costs $0 at any scale, works offline

Full `clawc` (Zig) compilation stays a local-install feature; the wasm
playground covers the teach-and-try loop (define, query, run, share).
Share links = code in the URL fragment — no storage backend at all.

Build order:
- ~~v0: wasm-pack claw-core + claw-cdb(in-memory) + claw-constraint;
  swap the playground's hand-written JS mirror for the real engine~~
  **done** — the wasm playground is live at clawlang.dev
- v1: examples menu (repo examples), share-by-URL, format
- v2: `claw test` semantics in-browser via contracts; embed on docs pages

### Also worth copying (cheap, high-signal)

- **pkg.go.dev's badge generator** → `claw badge` printing a shields URL
  with the verify state.
- **Rust playground's example menu** → our `examples/*.claw` are already
  the right content.
- **npm's provenance** → we sign content hashes; simpler and stronger.
