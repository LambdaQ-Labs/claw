# Contributing to Claw

Claw is an early, in-the-open research bet: **a programming language designed to be written and verified by machines.** That means there's a lot of high-leverage, well-scoped work available — and unusually clear ways to tell if a change is good (the benchmark moves, or it doesn't).

New here? The fastest way to understand the project is [`docs/master-plan.md`](docs/master-plan.md) (the whole plan) and [`docs/p2-spec.md`](docs/p2-spec.md) (the core idea — code-as-database + constrained decoding).

## Ways to help (roughly easiest → deepest)

### 🟢 Good first contributions
- **Add benchmark tasks.** The single most valuable low-barrier contribution. Each task is one JSON file in [`bench/tasks/`](bench/tasks) — a prompt, an in-scope symbol table, and grading rules. More tasks = more statistical weight behind every claim. See any existing task for the shape; `cargo test -p claw-bench-runner` validates new ones.
- **Break the grammar.** Find a Claw program the GBNF projection (`constraint-server/src/gbnf.rs`) mishandles, or a valid construct it can't express. Open an issue with the case.
- **Docs & examples.** Real `.claw` programs in [`examples/`](examples), clearer specs, fixing anything that reads wrong.

### 🟡 Meatier
- **CDB queries** (`cdb/`) — new ways to interrogate the code-as-database (better `search`, transitive deps, scoping).
- **Diagnostics** (`diagnostics/`) — richer structured errors + better ranked patches.
- **Runner backends** (`bench/runner/`) — new model integrations, better output-format handling.

### 🔴 Deep work (come talk to us first)
- **Compiler** (`compiler/`, the Roc fork, Zig) — `.claw` semantics, `clawc defs --json`, wiring the test runner so tasks actually *pass* not just compile.
- **Contracts & effects** — the P3 language layer (see the master plan).
- **The cold-start problem** — synthetic corpus + the bundled model (v1 ships in the release bundle; `claw ai`). Making it *better* — bigger corpora, the telemetry flywheel, stronger gates — is the hardest and most important open problem.

## Dev setup

```bash
git clone https://github.com/LambdaQ-Labs/claw && cd claw
cargo test --workspace          # Rust toolchain crates
cd compiler && zig build roc    # compiler (needs Zig 0.16.x) → clawc
```

## Ground rules

- **`cargo test --workspace` and `cargo clippy --workspace --all-targets` must be clean** before a PR. `cargo fmt --all` too.
- **Claims are backed by the benchmark.** If your change is supposed to help models, show the arm-over-arm delta. If it bounds coverage (caps, sampling), say so — no silent truncation.
- **Keep the honesty.** README and docs make strong claims *because they're true and cited*. Don't add a claim you can't point at data for.
- **Small, focused PRs.** One idea per PR. Explain the "why," not just the "what."

## Scope discipline (things we deliberately don't do)

Natural-language-as-source · a borrow checker · a from-scratch package ecosystem (we'll interop with Rust's) · human-ergonomics syntax bikeshedding. The agent is the author; humans review.

## Conduct

Be decent. Argue about ideas, not people. We especially welcome the argument about whether languages *should* be designed for machines — bring evidence.

---

Questions? Open a [Discussion](https://github.com/LambdaQ-Labs/claw/discussions) or an issue. Building this in the open is the point.
