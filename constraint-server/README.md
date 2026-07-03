# constraint-server/ — WS-C (Generation-Constraint Server)

Turns CDB + type context into a decode-time token mask so ill-typed / out-of-scope code is *ungeneratable*. The peer-reviewed >50% compile-error lever (PLDI 2025). Spec: [`../docs/p2-spec.md`](../docs/p2-spec.md) §2.

- Per decode step: hole → legal continuations (grammar ∩ typed `candidates` ∩ non-deprecated ∩ contract-valid) → token mask.
- Runs as a **separate process** with thin RPC; plugs into vLLM / llama.cpp as a logits processor.
- Empty mask → widen to grammar-only + emit WS-D diagnostic. Never hang the decoder.
- Speculative masking k tokens ahead where the grammar is deterministic.
