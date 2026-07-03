# Claw

**An AI-agent-first programming language.** By [LambdaQ Labs](https://clawlang.dev).

Claw is a language designed for LLM agents to write and verify — not for humans to hand-author. It forks [Roc](https://www.roc-lang.org) (memory-safe without a borrow checker, sound types, best-in-class errors) and adds the machinery the research says agentic coding actually needs:

- **Code-as-database** — source is a content-addressed store of definitions, not text files. Agents edit by hash, query real symbols, and *cannot* hallucinate APIs that don't exist.
- **Type-constrained generation** — ill-typed code is ungeneratable, not merely rejected (the peer-reviewed >50% compile-error lever).
- **Structured errors** — every diagnostic is machine JSON with ranked patches, for tight agent retry loops.
- **Contracts** — `requires`/`ensures`/`example` inline, to catch "compiles but does the wrong thing."
- **Effects & capabilities** — every effect visible in the type; nothing does I/O without a passed capability. Safe autonomous sandboxing by construction.
- **Rust interop** — FFI to any crate + `--emit=rust`, so Claw inherits the ecosystem instead of dying of isolation.

## Why

Every prior AI-first language died on **training-data cold-start + ecosystem**, not features. Claw's bet is 80% cold-start solve (bundled fine-tuned model + synthetic corpus + Rust interop), 20% language. See [`docs/master-plan.md`](docs/master-plan.md).

## Status

**Pre-P0.** Scaffolding. Nothing builds yet. The first real milestone is the benchmark harness + a baseline number — see [`docs/benchmark-harness.md`](docs/benchmark-harness.md).

## Repo layout

| Dir | Workstream | What |
|-----|-----------|------|
| `compiler/` | WS-A | The Roc fork — parser, type inference, backends (native + `--emit=rust`) |
| `cdb/` | WS-B | Code-as-database — content-addressed def store, symbol/dep queries |
| `constraint-server/` | WS-C | Generation-constraint server — type+scope token masks for the model |
| `diagnostics/` | WS-D | Structured-error protocol |
| `cli/` | WS-I | The `claw` CLI |
| `bench/` | WS-J | Benchmark harness + task set + auto-grader |
| `model/` | WS-H | Synthetic corpus engine + fine-tune pipeline |
| `examples/` | — | Sample `.claw` programs |
| `docs/` | — | Plan, specs, syntax |

## Docs

- [`docs/master-plan.md`](docs/master-plan.md) — the full 6-phase plan + kill-gates
- [`docs/p2-spec.md`](docs/p2-spec.md) — code-as-database + constraint-server deep spec (the make-or-break phase)
- [`docs/syntax.md`](docs/syntax.md) — language syntax sketch
- [`docs/benchmark-harness.md`](docs/benchmark-harness.md) — WS-J: how we measure everything
- [`docs/fork-strategy.md`](docs/fork-strategy.md) — how we track/diverge from upstream Roc

## License

TBD (Roc is UPL-1.0 — a permissive fork is compatible; decide before first public release).
