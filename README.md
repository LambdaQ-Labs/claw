<div align="center">

<img src="assets/achuk-logo.svg" width="88" alt="Achuk"><br>

# Achuk

### The programming language where **AI can't hallucinate APIs.**

*Not "hallucinates less." Can't. It's ungeneratable.*

[![status](https://img.shields.io/badge/status-experimental-orange)](#status-honest)
[![built on](https://img.shields.io/badge/forked%20from-Roc-a020f0)](https://www.roc-lang.org)
[![license](https://img.shields.io/badge/license-UPL--1.0-blue)](#license)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen)](#contributing)

**[Why](#the-idea) · [The Data](#the-data-real-not-vibes) · [Quickstart](#quickstart) · [How](#how-it-works) · [Status](#status-honest)**

</div>

---

Every LLM code assistant shares one dominant failure: it calls functions that **don't exist**. `generate_nonce()`, `list.sortBy()`, that method you *swear* the library has. In one study, **hallucinated APIs caused 41% of all compilation failures** in LLM-generated code.

Everyone else is fixing this with *bigger models* and *more retries*.

**Achuk fixes it at the language level.** The compiler exposes a live database of every real, in-scope symbol, and the model is **constrained at decode time** to only emit those. A function that doesn't exist isn't "discouraged" — it is literally not in the grammar. The model *cannot type it.*

## The data (real, not vibes)

Same 15 tasks. Same models. The only change: give the model Achuk's code-as-database symbol table.

| Model | Compiled ✗→✓ | Hallucinated symbols ✗→✓ |
|---|---|---|
| **DeepSeek-chat** | 0/15 → **13/15** | 38 → **0** |
| **Codestral** | 0/15 → **10/15** | 28 → **1** |

> API hallucination: **−96% to −100%**, from the language alone. No fine-tuning. No bigger model. [Full methodology →](docs/baseline-2026-07-03.md)

And with fine-tuning, the bundled model now clears the survival test. On the reference gate it is **121/121 (100%) hallucination-free and effect-sound** ([gate writeup](docs/p4-v3-gate-2026-07-05.md)) — at both 0.5B and 7B. On functional correctness (Pass@1, 116 tasks, execution-graded, same model per row):

| Model | Achuk (tuned) | JS | Python | Rust | Go |
|---|---|---|---|---|---|
| **0.5B** | **94%** | 89% | 56% | 35% | 7% |
| **7B** | **94%** (110/116) | 68% | 71% | 87% | 85% |

The same model writes Achuk better than it writes JavaScript, Python, Rust, or Go — at both scales. [Full parity writeup →](docs/parity-2026-07-05.md)

There's a stronger layer too: with decode-time grammar constraints, an out-of-scope **library API call is literally ungeneratable** — the symbol isn't in the model's grammar, so it can't be typed. (Bare unbound *locals* still need the typechecker; a context-free grammar can't tell a lambda param from a free var. We publish the [honest first A2 run](docs/baseline-2026-07-03.md), warts and all.)

## The idea

> A programming language designed to be **written by machines and verified by machines** — not typed by humans.

The research is blunt: [every prior "AI-first" language died](docs/master-plan.md) on training-data cold-start and ecosystem, not on ideas. So Achuk is engineered around the failure modes LLMs *actually* have, measured on real benchmarks:

- **🚫 No hallucinated APIs** — code-as-database + decode-time grammar constraints make out-of-scope calls *ungeneratable*
- **🧬 Code is a database, not text files** — content-addressed definitions; rename is O(1) and never breaks a caller
- **🔁 Structured errors, not prose** — every diagnostic is JSON with ranked patches, built for an agent's retry loop
- **🛡️ Memory-safe with no borrow-checker tax** — forked from [Roc](https://www.roc-lang.org): the strictness that helps LLMs, without the 92% compile-fail wall that Rust hits
- **📜 Contracts that execute** — `requires`/`ensures` are run on generated inputs, so "compiles" becomes "provably correct"
- **⚡ Effects & capabilities** — every effect visible in the type; a sandbox rejects ungranted I/O
- **🦀 Rust interop** — `emit-rust` lowers Achuk to compilable Rust, so you inherit crates.io instead of dying of isolation
- **🌱 A bundled model** — every install ships a fine-tuned model that already speaks Achuk (`achuk ai gen`), trained on a self-verifying synthetic corpus (the cold-start escape)

## Quickstart

Install the self-contained toolchain (no system compiler or linker needed).
It's **one bundle**: compiler, tooling, *and* the fine-tuned model with its
inference server — nothing else to download:

```bash
curl -fsSL https://achuk.dev/install.sh | sh          # macOS / Linux
```
```powershell
irm https://achuk.dev/install.ps1 | iex                # Windows (PowerShell)
```

Or try it without installing: the [playground](https://achuk.dev/playground.html)
runs the real engine (wasm) in your browser.

Write and run a program:

```bash
achuk new hello
cd hello
achuk run                 # → Hello, world!
```

Let the **bundled model** write Achuk for you — prompted from your project's
real symbols, grammar-constrained at decode time, and verified by the real
compiler before you see it:

```bash
achuk ai gen "define double : Nat -> Nat"
```

Or wire *your* agent in — grounded in your *real* code so it can't
invent APIs:

```bash
achuk mcp install         # registers the MCP server with Claude Code
achuk index               # (re)index your project's real symbols
```

Now Claude Code can call `achuk_symbols` / `achuk_candidates` / `achuk_mask` /
`achuk_render` / `achuk_check` and only ever reference functions that actually
exist.

Packages carry the same guarantee: the [registry](https://registry.achuk.dev)
rejects a publish without machine-readable definitions, and `achuk add` ingests
them into your project's database — so the AI knows an installed package's
names, types, and effects the moment it lands.

**New here?** Read [Getting started](docs/getting-started.md) and
[the language in 10 minutes](docs/tour.md), or browse runnable
[`examples/`](examples).

<details>
<summary>Building from source / the research toolchain</summary>

```bash
git clone https://github.com/LambdaQ-Labs/achuk && cd achuk
cargo test --workspace                 # the Rust toolchain — all green
cd compiler && zig build roc           # the compiler → achukc
sh scripts/package.sh v0.1.1           # build a release tarball → dist/

# the code-as-database, directly
cargo run -p achuk-cli -- db candidates "Nat, Nat -> a"
cargo run -p achuk-cli -- db mask "Nat, Nat -> a"   # the grammar that makes hallucination impossible

# benchmark any model (blind vs +Achuk's symbol table)
export ACHUK_MODEL_URL=… ACHUK_MODEL_NAME=… ACHUK_MODEL_KEY=…
cargo run -p achuk-bench-runner -- run --arm A1 --tasks bench/tasks

# transpile Achuk → Rust; generate a self-verifying training corpus
cargo run -p achuk-cli -- emit-rust defs.json
cargo run -p achuk-cli -- corpus gen --stdlib > corpus.jsonl
```

Or open [`playground/index.html`](playground/index.html) — an in-browser demo.

</details>

## What works today

| Capability | State |
|---|---|
| Compile & run real programs (self-contained: bundled platform + linker) | ✅ works |
| `achuk new` / `run` / project model | ✅ works |
| Print, compute, command-line args, `Str`/`Num`/`List` builtins | ✅ works |
| **Networking**: `achuk new --platform http` → a multi-request HTTP server ([verified](docs/networking.md)) | ✅ works (macOS + Linux) |
| `achuk new --platform cli` → stdin/stdout apps | ✅ works |
| AI guardrail over your **real** symbols — names, **types, and effects** (`achukc defs --json` → `achuk index` → MCP) | ✅ works |
| **Real call graph on real code** — `achuk db deps` / `callers` from lowered bodies (body-lowering: CIR → AST) | ✅ works |
| **Run + property-check your real code** — `achuk db eval` runs a def from the DB; `achuk db check` property-tests a contract against it | ✅ works |
| Decode-time grammar that makes out-of-scope calls ungeneratable | ✅ works (Def-JSON protocol) |
| Bundled fine-tuned model (**121/121 (100%)** hallucination-free + effect-sound, [P4 v3 gate](docs/p4-v3-gate-2026-07-05.md); **94%** functional Pass@1, [parity](docs/parity-2026-07-05.md)) — `achuk ai gen` ships in the bundle | ✅ works |
| **Packages**: `achuk publish` / `achuk add` against the [live registry](https://registry.achuk.dev); every package publishes its defs so the AI layer knows it on install | ✅ works |
| `emit-rust` on real bodies | 🧪 experimental |
| Records / tag-unions / `match` in lowered bodies (currently opaque markers) | 🗺️ roadmap (needs AST + interp support) |
| File / stdin platform I/O beyond print | 🗺️ roadmap (needs a new host) |
| Windows | 🟡 installer live (`install.ps1`); binaries cross-compiled, untested on real hardware |

The honest boundary: the language **runs today** (mac + linux, including a
real HTTP server), and the AI now understands your code at the level of
**names, types, effects, AND the call graph** — the compiler lowers each
checked body into the AST, so `achuk db deps`/`callers` answer over real,
even mutually-recursive, code. What's not yet lowered (records, tag unions,
`match`) becomes an opaque marker — extending that, and running contracts on
these real bodies, is the next step.

## How it works

```
 .achuk source ─► achukc (typecheck) ─► code-as-database ─► candidates(type) ─► grammar mask ─► model
                                          │                                         │
                                    real symbols only              out-of-scope calls ungeneratable
```

The load-bearing trick: the model never references a symbol by guessing its name. It picks from a **typed menu of things that provably exist** — and the decoder's grammar won't let it write anything else.

## Repo layout

| Crate / dir | What |
|---|---|
| `compiler/` | The compiler (`achukc`), forked from Roc — type-checks `.achuk` |
| `crates/achuk-core` | AST, content-addressed hashing, unification, a small interpreter, `.achuk` renderer |
| `cdb/` | **Code-as-database** — SQLite store, O(1) rename, type-directed `candidates()` |
| `constraint-server/` | The GBNF projection that makes out-of-scope calls ungeneratable |
| `diagnostics/` | Structured-error protocol (JSON + ranked patches) |
| `contract/` | Executable `requires`/`ensures` — predicate parser, evaluator, property gen |
| `effects/` | Effect-row inference + capability sandbox |
| `emit-rust/` | Achuk → Rust transpiler (ecosystem interop) |
| `corpus/` | Synthetic, self-verifying training-corpus generator (the cold-start seed) |
| `cli/` | The `achuk` CLI (db / compiler passthrough / `achuk ai` / publish + add / emit-rust / corpus) |
| `mcp/` · `lsp/` | MCP server (agents) and Language Server (editors) over the CDB |
| `bench/` | Benchmark harness — `tasks/` (31), `tasks-holdout/` (25), `tasks-large/` (121), `grammars/` (146), parity arms, grader with executable contracts |
| `train/` | LoRA fine-tune pipeline — `corpus-v4.jsonl`, four gate runs, parity harness |
| `telemetry/` | Usage telemetry (anonymous metrics by default, `achuk telemetry off`; code sharing opt-in) + collection worker |
| `model/` | Build staging for the bundled model (`achuk-0.5b-q8.gguf` + the `achuk-infer` llama.cpp server) — packaged into every release tarball |
| `editors/` | VS Code extension (tmLanguage grammar + snippets, packaged vsix) |
| `platforms/` | Bundled platforms (print, cli, http) for macOS arm64 + Linux musl |
| `examples/` · `scripts/` | Runnable examples · packaging + release scripts |
| `playground/` · `registry/` | In-browser playground (real engine, wasm) · the package registry service — both live at [achuk.dev](https://achuk.dev) / [registry.achuk.dev](https://registry.achuk.dev) |
| `site/` | The [achuk.dev](https://achuk.dev) website (serves `install.sh`, docs, and the playground) |
| `docs/` | Master plan, specs, and the honest benchmark writeups |

## Status (honest)

**Experimental. Pre-alpha. Built in the open** — but further than most first commits. What works today, with tests: the compiler type-checks `.achuk`; the code-as-database, constraint server, and structured errors run; contracts *execute* on generated inputs (behaviour-level pass, not just compile); effects + capabilities check; `emit-rust` and the MCP/LSP servers work; and a fine-tuned "bundled model" was trained end-to-end on a self-verifying corpus (for **$0.03** of GPU) and emits valid, in-scope Achuk. See the [benchmark writeup](docs/baseline-2026-07-03.md) — warts and all.

The survival test — does the tuned model beat a general model on Achuk? — **passed on 2026-07-05, at both scales**: 121/121 (100%) hallucination-free + effect-sound on the reference gate, and 94% functional Pass@1 vs 89% JS / 56% Python (0.5B) and 94% vs 87% Rust / 85% Go / 71% Python / 68% JS (7B). See the [gate](docs/p4-v3-gate-2026-07-05.md) and [parity](docs/parity-2026-07-05.md) writeups.

The ecosystem is live: [achuk.dev](https://achuk.dev) (site, playground, installer), [registry.achuk.dev](https://registry.achuk.dev) (packages), and the model ships **in** the install bundle (`achuk ai`). What's next: a bigger holdout, records in the type system, and launch. This is a research bet with real, measured evidence — not a finished product. If that's your kind of thing — **★ star it and watch where it goes.**

## Contributing

Issues, ideas, and PRs welcome — especially benchmark tasks, grammar edge cases, and compiler work. Good first issues are tagged. Come argue with us about whether languages should be designed for humans or machines.

## License

UPL-1.0 (matching upstream Roc). Built by [LambdaQ Labs](https://achuk.dev).

<div align="center">

*If a language where the AI **cannot** invent a fake API sounds interesting — the ★ button is right up there.*

</div>
