<div align="center">

# 🐾 Claw

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

**Claw fixes it at the language level.** The compiler exposes a live database of every real, in-scope symbol, and the model is **constrained at decode time** to only emit those. A function that doesn't exist isn't "discouraged" — it is literally not in the grammar. The model *cannot type it.*

## The data (real, not vibes)

Same 15 tasks. Same models. The only change: give the model Claw's code-as-database symbol table.

| Model | Compiled ✗→✓ | Hallucinated symbols ✗→✓ |
|---|---|---|
| **DeepSeek-chat** | 0/15 → **13/15** | 38 → **0** |
| **Codestral** | 0/15 → **10/15** | 28 → **1** |

> API hallucination: **−96% to −100%**, from the language alone. No fine-tuning. No bigger model. [Full methodology →](docs/baseline-2026-07-03.md)

There's a stronger layer too: with decode-time grammar constraints, an out-of-scope **library API call is literally ungeneratable** — the symbol isn't in the model's grammar, so it can't be typed. (Bare unbound *locals* still need the typechecker; a context-free grammar can't tell a lambda param from a free var. We publish the [honest first A2 run](docs/baseline-2026-07-03.md), warts and all.)

## The idea

> A programming language designed to be **written by machines and verified by machines** — not typed by humans.

The research is blunt: [every prior "AI-first" language died](docs/master-plan.md) on training-data cold-start and ecosystem, not on ideas. So Claw is engineered around the failure modes LLMs *actually* have, measured on real benchmarks:

- **🚫 No hallucinated APIs** — code-as-database + decode-time grammar constraints make out-of-scope calls *ungeneratable*
- **🧬 Code is a database, not text files** — content-addressed definitions; rename is O(1) and never breaks a caller
- **🔁 Structured errors, not prose** — every diagnostic is JSON with ranked patches, built for an agent's retry loop
- **🛡️ Memory-safe with no borrow-checker tax** — forked from [Roc](https://www.roc-lang.org): the strictness that helps LLMs, without the 92% compile-fail wall that Rust hits
- **📜 Contracts that execute** — `requires`/`ensures` are run on generated inputs, so "compiles" becomes "provably correct"
- **⚡ Effects & capabilities** — every effect visible in the type; a sandbox rejects ungranted I/O
- **🦀 Rust interop** — `emit-rust` lowers Claw to compilable Rust, so you inherit crates.io instead of dying of isolation
- **🌱 A bundled model** — a fine-tuned model that already speaks Claw, trained on a self-verifying synthetic corpus (the cold-start escape)

## Quickstart

Install the self-contained toolchain (no system compiler or linker needed):

```bash
curl -fsSL https://clawlang.dev/install.sh | sh
```

Write and run a program:

```bash
claw new hello
cd hello
claw run                 # → Hello, world!
```

Let an AI agent write Claw for you — grounded in your *real* code so it can't
invent APIs:

```bash
claw mcp install         # registers the MCP server with Claude Code
claw index               # (re)index your project's real symbols
```

Now Claude Code can call `claw_symbols` / `claw_candidates` / `claw_mask` and
only ever reference functions that actually exist.

**New here?** Read [Getting started](docs/getting-started.md) and
[the language in 10 minutes](docs/tour.md), or browse runnable
[`examples/`](examples).

<details>
<summary>Building from source / the research toolchain</summary>

```bash
git clone https://github.com/LambdaQ-Labs/claw && cd claw
cargo test --workspace                 # the Rust toolchain — all green
cd compiler && zig build roc           # the compiler → clawc
sh scripts/package.sh v0.1.0           # build a release tarball → dist/

# the code-as-database, directly
cargo run -p claw-cli -- db candidates "Nat, Nat -> a"
cargo run -p claw-cli -- db mask "Nat, Nat -> a"   # the grammar that makes hallucination impossible

# benchmark any model (blind vs +Claw's symbol table)
export CLAW_MODEL_URL=… CLAW_MODEL_NAME=… CLAW_MODEL_KEY=…
cargo run -p claw-bench-runner -- run --arm A1 --tasks bench/tasks

# transpile Claw → Rust; generate a self-verifying training corpus
cargo run -p claw-cli -- emit-rust defs.json
cargo run -p claw-cli -- corpus gen --stdlib > corpus.jsonl
```

Or open [`playground/index.html`](playground/index.html) — an in-browser demo.

</details>

## What works today

| Capability | State |
|---|---|
| Compile & run real programs (self-contained: bundled platform + linker) | ✅ works |
| `claw new` / `run` / project model | ✅ works |
| Print, compute, command-line args, `Str`/`Num`/`List` builtins | ✅ works |
| AI guardrail over your **real** symbols (`claw index` + MCP: symbols/candidates/mask) | ✅ works |
| Decode-time grammar that makes out-of-scope calls ungeneratable | ✅ works (Def-JSON protocol) |
| HTTP server / networking via an explicit platform ([verified auth gateway](docs/networking.md)) | 🧪 works (macOS; not yet a first-class `claw new` target) |
| Bundled fine-tuned model (0→98% hallucination-free, [P4 gate](docs/p4-gate-2026-07-04.md)) | 🧪 research (separate download) |
| Contracts / effects / `emit-rust` | 🧪 experimental (synthetic AST) |
| Networking/file I/O as a bundled `claw new --platform` target | 🗺️ roadmap (v0.1.1) |
| AI understands whole programs (bodies, call-graph, contracts on your code) | 🗺️ roadmap (v0.2) |
| Windows | 🗺️ roadmap |

The honest boundary: the language **runs today**, and the AI guardrail works
at the **symbol level** on your real code. Deeper program understanding
(lowering real bodies + call-graph into the database) is the v0.2 work.

## How it works

```
 .claw source ─► clawc (typecheck) ─► code-as-database ─► candidates(type) ─► grammar mask ─► model
                                          │                                         │
                                    real symbols only              out-of-scope calls ungeneratable
```

The load-bearing trick: the model never references a symbol by guessing its name. It picks from a **typed menu of things that provably exist** — and the decoder's grammar won't let it write anything else.

## Repo layout

| Crate / dir | What |
|---|---|
| `compiler/` | The compiler (`clawc`), forked from Roc — type-checks `.claw` |
| `crates/claw-core` | AST, content-addressed hashing, unification, a small interpreter, `.claw` renderer |
| `cdb/` | **Code-as-database** — SQLite store, O(1) rename, type-directed `candidates()` |
| `constraint-server/` | The GBNF projection that makes out-of-scope calls ungeneratable |
| `diagnostics/` | Structured-error protocol (JSON + ranked patches) |
| `contract/` | Executable `requires`/`ensures` — predicate parser, evaluator, property gen |
| `effects/` | Effect-row inference + capability sandbox |
| `emit-rust/` | Claw → Rust transpiler (ecosystem interop) |
| `corpus/` | Synthetic, self-verifying training-corpus generator (the cold-start seed) |
| `cli/` | The `claw` CLI (db / compiler / emit-rust / corpus) |
| `mcp/` · `lsp/` | MCP server (agents) and Language Server (editors) over the CDB |
| `bench/` | Benchmark harness — arms A0/A1/A2, grader with executable contracts |
| `train/` | LoRA fine-tune pipeline + the first bundled-model run |
| `playground/` · `registry/` | In-browser demo · content-addressed package format |
| `docs/` | Master plan, specs, and the honest benchmark writeups |

## Status (honest)

**Experimental. Pre-alpha. Built in the open** — but further than most first commits. What works today, with tests: the compiler type-checks `.claw`; the code-as-database, constraint server, and structured errors run; contracts *execute* on generated inputs (behaviour-level pass, not just compile); effects + capabilities check; `emit-rust` and the MCP/LSP servers work; and a fine-tuned "bundled model" was trained end-to-end on a self-verifying corpus (for **$0.03** of GPU) and emits valid, in-scope Claw. See the [benchmark writeup](docs/baseline-2026-07-03.md) — warts and all.

What's next: scale the corpus so the bundled model *beats* a general model on Claw (the survival test), a real standard library, and adoption. This is a research bet with real, measured evidence — not a finished product. If that's your kind of thing — **★ star it and watch where it goes.**

## Contributing

Issues, ideas, and PRs welcome — especially benchmark tasks, grammar edge cases, and compiler work. Good first issues are tagged. Come argue with us about whether languages should be designed for humans or machines.

## License

UPL-1.0 (matching upstream Roc). Built by [LambdaQ Labs](https://clawlang.dev).

<div align="center">

*If a language where the AI **cannot** invent a fake API sounds interesting — the ★ button is right up there.*

</div>
