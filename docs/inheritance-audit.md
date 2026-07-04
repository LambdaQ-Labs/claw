# Claw ⟵ Roc: inheritance audit

Claw is a fork of the Roc compiler (Zig). This is the honest map of **what
Claw gets for free from Roc**, **what Claw adds** (the AI-first value),
**what still leaks the `roc` brand**, and **where we duplicated something the
compiler already had**. Kept current as the project evolves.

## 1. Inherited from Roc (free — it *is* a Roc fork)

| Capability | Notes |
|---|---|
| The language | syntax, Hindley-Milner type inference, tag unions, records, `match`, modules, `\|x\|` lambdas |
| The compiler | parse → canonicalize → typecheck → codegen (dev interpreter + LLVM), **embedded LLD linker** |
| Stdlib builtins | Str, Num, List, Bool, Dict, Set, Result, Try |
| Platforms + host FFI | the platform model; bundled echo / fx / http hosts |
| Package system | content-addressed `.tar.zst` bundles, URL packages, download+cache+**BLAKE3 hash-verify**, `import pkg.Module` |
| Effects / purity | `!` effectful functions; `fn_pure`/`fn_effectful` (read by `defs --json`) |
| Tooling | `check/build/run/test/repl/fmt/docs`, snapshot tests, LSP, cross-compile targets |

The **package registry + `publish`/`add`** are built *on top* of Roc's
bundle+URL machinery — additive, not reinvented.

## 2. Claw-original (none of this is Roc — the reason Claw exists)

- **code-as-database** — SQLite store, content-addressed defs, O(1) rename, type-directed `candidates()`
- **constraint-server / GBNF** — decode-time grammar that makes out-of-scope calls *ungeneratable*
- **`clawc defs --json`** + **body-lowering** — CIR → serializable AST → the CDB's real **call graph** (`db deps`/`callers`/`eval`/`check`)
- **executable contracts** — predicate language + property-check on real code (`db check`)
- **structured diagnostics** — JSON + ranked patches
- **corpus generator + bundled-model training** — the cold-start escape (0→98% hallucination-free)
- **benchmark harness** — A0/A1/A2 arms + grader
- **MCP server** — agent grounding over the real symbol table
- **emit-rust** — Claw → Rust transpiler
- **package registry service** + `claw publish` / `claw add`

## 3. Rebrand status

**Fixed:**
- package cache dir `~/.cache/roc` → `~/.cache/claw`; temp dir `{tmp}/roc` → `{tmp}/claw`
- CLI help/usage `roc <cmd>` → `claw <cmd>`, `ROC_FILE` → `CLAW_FILE`, reporter labels
- `clawc version` string, README/docs

**Still leaks `roc` (known, mostly internal):**
- **Library module files resolve as `.roc`, not `.claw`** — the module loader hardcodes `.roc` across 6+ sites (`compile_package.zig`, `compile_build.zig`, `coordinator.zig`). `.claw` is the app-facing extension; `.roc` the internal one. Same language. Unifying is a real change, deferred.
- Host/ABI symbols: `roc_main`, `roc_builtins`, `.roc_echo_platform`, generated `main.roc` — internal, not user-facing.
- No Claw platform *ecosystem* — example platform URLs still point at roc-lang.
- `.tar.br` strings in a few legacy test fixtures (dead text; the code path is `.tar.zst`).

## 4. Duplication — where we built what the compiler already has

| Doubled | Roc already has | Why | Verdict |
|---|---|---|---|
| `claw-core::interp` (Expr interpreter) | Roc's dev-backend interpreter (`clawc <file>`) | run CDB bodies for `db eval`/`check`/contracts without the full compiler | **real double** |
| `claw-core::Expr` (small AST) | the compiler's CIR | CDB needs a serializable, content-addressable, agent-facing AST | **real double** (body-lowering is the CIR→Expr bridge over it) |
| `parse_type` (type mini-parser) | full type inference | re-parse type strings for CDB queries | **real double** |
| `contract::eval::Value` vs `interp::Value` | — | two value types + a converter | minor internal double |

**Not doubles** (correctly additive): `run`/`build`/`check`/`fmt` (thin
passthrough to `clawc`), `publish`/`add` (Roc has no registry/UX), emit-rust.

### The architectural smell
The **interp + toy-AST + type-mini-parser** form a *parallel
evaluation/representation stack* mirroring the compiler's CIR + typechecker +
dev-interpreter. Deliberate (the CDB wanted its own serializable AST), but the
long-term right design is to make the **compiler's CIR *be* the CDB
representation** — then `db eval` uses Roc's real interpreter and the double
collapses. See the "collapse a double" work: `db eval --real` runs a def
through the actual compiler instead of the toy interp, as the first step.

## 5. Genuinely missing (not built)

- Native DB hosts (Postgres, …) — new Zig hosts (libpq/sockets + ABI). Week+ each.
- Rust-library FFI (call Rust *from* Claw) — host FFI work. (`emit-rust` does the reverse.)
- File / stdin I/O as first-class (only via platforms today).
- Public registry (HTTPS + hosting); private/authed registries (no client auth hook).
- Bundled model shipped in the tarball; Windows target verification.
