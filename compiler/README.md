# compiler/ — WS-A (Roc fork)

The Claw compiler, vendored from [Roc](https://github.com/roc-lang/roc). See [`../docs/fork-strategy.md`](../docs/fork-strategy.md).

**Pre-P0 — empty.** Bootstrap:
1. Vendor Roc source here (`rsync` per fork-strategy).
2. Record fork point in `UPSTREAM.md`.
3. Run the Roc→Claw rename pass.
4. `claw build hello.claw` → native binary green = P0 exit.

Responsibilities: parser, type inference (sound HM — the substrate the constraint server needs), effect inference, native backend (LLVM), and the `--emit=rust` transpile backend (WS-G).
