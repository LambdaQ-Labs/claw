# Claw — Benchmark Harness Spec (WS-J)

*Build this FIRST — before compiler features, before the constraint server. You cannot steer the project without a number, and both kill-gates (P2, P4) are defined against this harness. It is the single most important piece of infrastructure in the plan.*

---

## 0. Why first

The research that justifies Claw is all quantitative (RustRepoTrans, Multi-SWE-bench, PLDI type-constrained decoding). The whole bet reduces to: *can we move a measured number?* If we can't measure, we're vibing, not engineering. The harness turns every later decision (does the constraint server help? did the bundled model beat Python?) into a gated, falsifiable experiment.

---

## 1. What it measures

**Primary:** iterative Pass@1 of an LLM agent on repository-level Claw tasks.
**Secondary:** compile-error rate, API-hallucination rate, tokens-to-green, wall-time.

"Iterative" = the agent gets ≤N compile/test feedback rounds (default N=3), because that's the real vibe-coding workflow — not single-shot. (Single-shot Pass@1 is also recorded for comparison to published benchmarks.)

---

## 2. Task set

Target **~200 tasks** at P0, growing. Each task is self-contained and auto-gradable.

### 2.1 Task categories (mix)
| Category | ~% | What it stresses |
|---|---|---|
| From-scratch function w/ real deps | 30% | code-as-DB symbol binding, types |
| Translate C/Rust/Python → Claw | 30% | comparability to RustRepoTrans; corpus source |
| Repo-level feature (edit N defs across "files") | 25% | cross-file context — the dominant failure axis |
| Contract-satisfying impl | 10% | intent misalignment (P3+) |
| Effect/capability-correct impl | 5% | effect system (P3+) |

### 2.2 Task schema
```json
{
  "id": "wallet-transfer-001",
  "category": "repo-feature",
  "prompt": "Implement transfer respecting the Ledger invariant.",
  "context": { "cdb_snapshot": "tasks/wallet-transfer-001/snapshot.cdb" },
  "grade": {
    "compile": true,
    "tests": ["tests/transfer_spec.claw"],
    "contracts": ["from'.balance == from.balance - amt"],
    "forbidden": ["unsafe", "hallucinated-symbol"]
  },
  "reference": "solutions/wallet-transfer-001.claw"
}
```

### 2.3 Sourcing tasks (bootstrap without a Claw corpus)
- **Translate** existing RustRepoTrans / CoderEval tasks into Claw task shells (reuse their test oracles).
- **Author** ~50 hand-written repo-level tasks from real small programs (wallet, parser, CLI arg-handling, HTTP handler).
- **Generate** synthetic tasks from the property-based corpus engine once WS-H exists (feeds back in).
- Keep a **held-out split** never used for model fine-tuning (leakage kills the P4 measurement).

---

## 3. The grader (`bench/grader`)

Deterministic, no model in the loop. Given an agent's produced CDB state:

```
grade(task, produced_cdb) -> {
  compiled:        bool,
  tests_passed:    int / total,
  contracts_held:  int / total,
  forbidden_hit:   [rule],
  hallucinated_symbols: [name],   # referenced a symbol not in the CDB scope
  pass:            bool,          # compiled ∧ all tests ∧ all contracts ∧ no forbidden
  retries_used:    int,
  tokens:          int,
}
```

- **Hallucination detection** = any symbol the produced code references that `cdb.symbols(scope)` doesn't contain. This is the metric that should go to ~0 once the constraint server lands — the headline P2 result.
- Grading is a pure function of (task, produced_cdb). Reproducible, CI-runnable.

---

## 4. The runner (arms / ablations)

The harness runs each task under multiple **arms** so we can attribute wins to specific subsystems:

| Arm | CDB context | Constraint mask | Structured errors | Model |
|-----|:-:|:-:|:-:|-------|
| A0 baseline | ✗ | ✗ | prose only | stock |
| A1 +context | ✓ | ✗ | prose | stock |
| A2 +mask (P2 thesis) | ✓ | ✓ | JSON | stock |
| A3 +bundled model (P4) | ✓ | ✓ | JSON | Claw fine-tune |
| Ref Python | n/a | n/a | n/a | stock on Python |

- **P2 gate:** `A2.compile_error_rate` must be **>30% lower** than `A0`, and `A2.hallucinated_symbols → ~0`.
- **P4 gate:** `A3.pass_rate(Claw) >= Ref.pass_rate(Python)` on the held-out split. ← the Matthew-Effect reversal.

---

## 5. CLI + CI

```
claw-bench run --arm A2 --tasks bench/tasks --model <endpoint> --retries 3
claw-bench report            # tables: pass@1, compile-err %, halluc %, tokens, by category
claw-bench compare A0 A2     # the gate delta
```

- Runs in CI on every compiler/CDB/CS change → regression guard. A change that lowers Pass@1 is a red build.
- Results archived per commit → a time-series of the number that matters, published at clawlang.dev once public.

---

## 6. Build order
1. Task schema + 10 hand-authored tasks + the deterministic grader.
2. Runner with arm A0 (stock model, no Claw infra) → **record the baseline number**. This is the P0 exit gate.
3. Add arms A1/A2 as CDB (WS-B) and CS (WS-C) come online in P2.
4. Grow to ~200 tasks; wire into CI.
5. Add A3 + Ref-Python + held-out split for the P4 gate.

## 7. Pitfalls to avoid
- **Leakage:** never fine-tune on held-out tasks. Separate splits from day one.
- **Gaming compile:** a task that compiles but does nothing must fail on tests/contracts — that's why grading is multi-signal, not compile-only.
- **Silent truncation:** if a run caps retries or samples a subset, the report must say so. A partial run that reads as "100% covered" is a lie that will cost you a gate decision.
