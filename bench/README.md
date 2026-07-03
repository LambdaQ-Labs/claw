# bench/ — WS-J (Benchmark Harness)

**Build this FIRST.** Both kill-gates (P2, P4) are defined against it. Full spec: [`../docs/benchmark-harness.md`](../docs/benchmark-harness.md).

- `tasks/` — ~200 repo-level tasks (JSON schema + reference solutions + test oracles).
- `grader/` — deterministic, model-free grader: compile ∧ tests ∧ contracts ∧ no-forbidden ∧ no-hallucinated-symbols.
- Runner arms: A0 baseline → A1 +context → A2 +mask (P2 gate) → A3 +bundled model / Ref-Python (P4 gate).

Gates:
- **P2:** A2 compile-error rate >30% below A0; hallucinated-symbols → ~0.
- **P4:** A3 pass-rate on Claw ≥ Ref pass-rate on Python (held-out split). The Matthew-Effect reversal.
