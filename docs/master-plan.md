# Claw — Master Implementation Plan

**Org:** LambdaQ Labs · **Language:** Claw · **Site:** clawlang.dev (+ .org) · **CLI:** `claw` · **File ext:** `.claw` · **Fork base:** Roc

---

## 0. First principles (locked by research)

- **Fork base = Roc.** Memory-safe without a borrow checker (refcount + opportunistic in-place mutation), effect-platforms = capabilities for free, best-in-class error messages, sound Hindley-Milner types (required for type-constrained decoding), small hackable compiler in Rust/Zig.
- **Success = 80% cold-start solve, 20% language.** Every prior AI-first language (Pel, Universalis, Darklang) died on training-data cold-start + ecosystem, NOT features. Resource the 80% accordingly.
- **Primary metric:** iterative Pass@1 for an LLM agent on repository-level tasks. Not human ergonomics — the agent is the author, humans review.
- **Two decision gates decide the whole bet:** P2 (is the thesis real?) and P4 (did we beat the Matthew Effect?). Everything before P2 is cheap; everything after P2 is earned.

### What the research proved we must fix (shortcomings → design levers)
| Shortcoming (evidence) | Claw lever |
|---|---|
| API hallucination = 41% of compile fails | Code-as-database: agent binds against real symbol table, can't emit non-existent calls |
| Repo-context collapse (GPT-4 85%→29%) | Dependency graph as first-class query, not text reconstruction |
| 92–95% Rust fails = compile errors (borrow-checker tax) | Roc base: no borrow checker; keep safety, drop the tax |
| Ill-typed generation | Type-constrained decoding (peer-reviewed >50% compile-error cut, PLDI 2025) |
| "Compiles but wrong" / intent misalignment | First-class contracts (pre/post/invariant) |
| Errors caught too late, prose-only | Structured-error protocol (JSON + ranked patches) |
| Cold-start (Matthew Effect) | Bundled fine-tuned model + synthetic corpus |
| Ecosystem death (Darklang) | Rust FFI + `--emit=rust` transpile → inherit cargo |

---

## 1. Workstreams

| WS | Name | Deliverable | Depends on |
|----|------|-------------|-----------|
| **A** | Language core (Roc fork) | Renamed compiler; native + IR backends | — |
| **B** | Code-as-database | Content-addressed def store; symbol/dep query; edit-by-hash | A |
| **C** | Generation-constraint server | Type+scope automaton API for the model | A, B |
| **D** | Structured-error protocol | JSON diagnostics + ranked patches | A |
| **E** | Contracts | pre/post/invariant + executable examples; static + property fallback | A |
| **F** | Effects & capabilities | Sharpen Roc platforms into cap-gated effects + sandbox | A |
| **G** | Interop / backends | Rust FFI + `--emit=rust` transpile | A |
| **H** | Model + corpus | Synthetic corpus engine + bundled fine-tuned model | B, C, D |
| **I** | Tooling | `claw` CLI, LSP, MCP server, formatter | A, B, D |
| **J** | Benchmark harness | Held-out repo-level task set, auto-graded Pass@1, CI | A, D |
| **K** | Docs / community | Spec, playground, registry, examples | all |

**Critical path: A → B → C → H.** That chain IS the thesis. Everything else supports it.

---

## 2. Phased timeline + gates

### P0 — Bootstrap (weeks 0–4)
Goal: a building, renamed Roc fork LambdaQ controls.
- Fork Roc → LambdaQ Labs monorepo, CI, contributor tooling. Keep upstream remote for selective merges.
- Global rename Roc → Claw (compiler, CLI, stdlib prefix, `.roc`→`.claw`).
- `claw build hello.claw` → native binary green.
- **Stand up WS-J benchmark harness NOW** — ~200 repo-level tasks (translate + from-scratch), auto-graded by compile + test. Record baseline: stock model on stock Roc.
- **Exit gate:** clean build, CI green, baseline Pass@1 number recorded. *You cannot steer without this number.*

### P1 — Feedback loop (weeks 4–10)
Goal: the agent loop *feels* different. Cheapest high-value wins.
- **WS-D structured errors:** every diagnostic → JSON `{hash-loc, category, expected, got, minimal_constraint, ranked_patches, render}`. Prose = a rendering of the struct.
- **WS-I:** `claw` CLI with an agent retry-loop that consumes WS-D structs; LSP MVP; formatter (uniform output → cleaner future training data).
- Fast incremental `claw check` (<100ms target).
- **Exit gate:** agent-with-error-loop beats agent-without on the benchmark. Proves the harness discriminates.

### P2 — The thesis (weeks 10–20) ← MAKE-OR-BREAK
Goal: prove code-as-DB + constrained decoding beat a plain model.
- **WS-B code-as-database:** content-addressed definition store; name→hash layer; symbol table + dependency graph as first-class queries; edit-by-hash; `claw db` commands. Backing: SQLite + content store to start.
- **WS-C constraint server:** given cursor + type context, emit automaton of well-typed, in-scope, non-deprecated next tokens, bound to WS-B symbol table. Agent physically cannot emit `generate_nonce()` if it's not in the DB.
- Wire a stock open code model (7–32B) to decode against WS-C (logits mask / grammar via vLLM or llama.cpp grammar hooks).
- **⛔ FAIL-CHEAP GATE:** measure compile-error rate + Pass@1, constrained vs unconstrained, on the P0 benchmark. **Need a clear win — target >30% compile-error reduction** (PLDI-proven ceiling is >50%). No win → STOP or re-architect before building anything heavier. **This gate decides the project.**

### P3 — Correctness layer (weeks 20–32)
Goal: catch "compiles but wrong" — intent.
- **WS-E contracts:** parse pre/post/invariant + executable examples; static-check where decidable; property-test generation otherwise.
- **WS-F effects/capabilities:** effects in type signatures; cap-gated I/O; sandbox runner for safe autonomous agent execution.
- Extend WS-C to constrain against in-scope contracts (don't generate calls that violate a known precondition).
- **Exit gate:** measurable drop in intent-misalignment failures (semantic pass, not just compile).

### P4 — Ecosystem escape (weeks 28–44, overlaps P3)
Goal: don't die like Darklang. Inherit the world + beat cold-start.
- **WS-G:** Rust FFI (call any crate) + `--emit=rust` backend (any Claw module = a normal Rust dep). Prove on 3 real crates.
- **WS-H model + corpus:** synthetic corpus engine — (1) transpile large Rust/TS repos → Claw pairs; (2) property-generate valid programs from types+contracts; (3) self-play: model generates → compiler labels compile/contract pass → SFT/RL feedback. Ship the **first bundled fine-tune** versioned with the toolchain.
- **Exit gate:** bundled-model-on-Claw Pass@1 ≥ stock-model-on-Python on the same repo-level tasks. **← the Matthew-Effect reversal. The real proof a new AI-first language can work.**

### P5 — Public alpha (weeks 44–56)
- **WS-K:** spec v1, docs, web playground, package registry (or federate onto crates), example gallery.
- MCP server GA — any external agent (Claude Code, etc.) can drive Claw.
- Design-partner program: 3–5 teams on real agent workloads.
- **Exit gate:** external users complete real tasks; retention signal; publish benchmark results at clawlang.dev.

---

## 3. Technical detail — critical-path pillars

### WS-B Code-as-database
- Each top-level def content-addressed (hashed, Unison-style). Name→hash mapping for humans/agents.
- Queries: `symbols(scope)`, `callers(hash)`, `deps(hash)`, `type_at(cursor)`, `search(sig)`.
- Edits: patch ONE def by hash; rename = pure metadata; no file re-parse. Kills whole-file token waste + repo-context reconstruction.
- Start SQLite + content store; graph queries over it. Don't over-engineer before P2.

### WS-C Constraint server
- API: `next_tokens(context) -> automaton` — valid continuations = well-typed ∧ in-scope (WS-B) ∧ contract-satisfying (WS-E).
- Integrate as logits mask / grammar at decode time.
- The peer-reviewed >50% lever. Core investment, not a bolt-on.

### WS-D Structured errors
- Schema: `{loc: hash+span, code, category, expected, got, minimal_constraint, patches:[ranked], render:string}`.
- Every compiler error path emits the struct; prose derived. Agent reads struct → applies top patch → re-checks.

### WS-H Model + corpus (the 80%)
- Corpus: transpiled Rust/TS pairs + property-gen valid programs + compiler-labeled self-play.
- Model: fine-tune an open 7–14B code model; ship it versioned with the toolchain; retrain each release on the grown corpus.
- The ONLY escape from cold-start. Under-resourcing this = the graveyard.

---

## 4. Team / skills

| Role | For |
|---|---|
| Compiler eng (Rust, type systems) ×2 | WS-A/B/C/E/F. Roc/OCaml/Haskell background ideal. |
| ML eng (LLM fine-tune, constrained decoding) ×2 | WS-C/H — thesis + cold-start. |
| Tooling eng (LSP, CLI, DX) ×1 | WS-I. |
| Infra/eval eng ×1 | WS-J harness, corpus pipeline, CI. |
| DevRel/docs ×1 (from P4) | WS-K adoption. |

Lean core to reach P2 gate = **2 compiler + 2 ML** (+ infra). Scale only after the gate passes.

---

## 5. Risks + kill criteria

| Risk | Mitigation | Kill trigger |
|---|---|---|
| Thesis wrong (constrained decode + code-as-DB don't beat plain model) | P2 fail-cheap gate before heavy build | P2 gate fails → stop |
| Cold-start unbeatable (bundled model still < Python-on-stock) | Synthetic corpus + self-play; P4 gate | P4 fails twice → pivot to "agent layer on Rust," not a new lang |
| Roc upstream churn / fork divergence | Track upstream, keep changes modular, contribute back | — |
| Compiler complexity underestimated | Start from Roc (small), not rustc; scope discipline | scope creep → defer contracts to P3+ |
| Scope creep (NL-source, fancy IDE) | Locked "don't build" list | — |

---

## 6. What we DON'T build (scope discipline)
Natural-language-as-source · borrow checker · new package ecosystem from scratch · human-ergonomics syntax debates · custom model architecture (fine-tune existing only).

---

## 7. Success metrics (the only ones that matter)
1. **P2:** compile-error reduction, constrained vs unconstrained (target >30%).
2. **P3:** semantic/intent-pass rate up (contracts catch wrong-but-compiles).
3. **P4:** bundled-model-on-Claw Pass@1 ≥ stock-model-on-Python, repo-level. ← the reversal that proves the whole bet.
4. **P5:** external design-partner task completion + retention.

Hit metric #3 and you've beaten the Matthew Effect — the thing that killed every prior AI-first language. That is the win condition.

---

## 8. Immediate next actions (this week)
1. Clone Roc → LambdaQ Labs monorepo + CI.
2. Global rename pass → Claw (`.roc`→`.claw`, `roc`→`claw` CLI).
3. **Build the benchmark harness FIRST** (~200 repo-level tasks, auto-graded) + record baseline (stock model, stock Roc).
4. Land the structured-error protocol (WS-D) — fastest felt improvement.
5. Point clawlang.dev at a holding page; clawlang.org → redirect.
