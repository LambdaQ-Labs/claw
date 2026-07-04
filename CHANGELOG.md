# Changelog

## v0.1.0 — first downloadable release (2026-07-04)

The release where Claw becomes something you can **download and build with**,
not just a research toolchain.

### Added
- **Install in one line:** `curl -fsSL https://clawlang.dev/install.sh | sh`
  installs a self-contained toolchain into `~/.claw` (bundled compiler,
  platform, and linker — no system toolchain required).
- **Project model:** `claw new <name>` scaffolds a runnable project;
  `claw run [file]` compiles and runs it.
- **AI guardrail on your real code:** `claw index` ingests a project's real
  functions + inferred types into the code-as-database; `claw mcp install`
  registers an MCP server so Claude Code (and any MCP client) can call
  `claw_symbols` / `claw_candidates` / `claw_mask` over *your* symbols and
  cannot reference APIs that don't exist.
- **Distribution:** `scripts/package.sh` builds per-platform tarballs; a
  GitHub Actions release workflow builds + smoke-tests + publishes for
  macOS (arm64) and Linux (x64).
- **Docs & examples:** getting-started, a 10-minute language tour, and
  runnable examples (hello, fizzbuzz, pattern matching, args).
- `claw --version`.

### Fixed
- 20 findings from a multi-agent code review across the toolchain
  (interpreter stack-overflow guard, checked arithmetic, type-variable
  capture in `candidates()`, GBNF canonical integers, emitter keyword
  escaping, distillation gate, and more).

### Known limits (roadmap)
- I/O is print + compute + args only; file/stdin/network is v0.1.1.
- The AI guardrail is symbol-level; lowering real bodies + call-graph into
  the database (so the AI understands whole programs) is v0.2.
- Contracts / effects / `emit-rust` operate on the synthetic AST, not yet
  on real `.claw` bodies.
- The bundled fine-tuned model ships as a separate research download.
- Windows is not yet a release target.

### Research
- First base-vs-tuned P4 gate: a fine-tuned 0.5B went **0 → 98%
  hallucination-free** on the target distribution for ~$0.30 of GPU, while
  its own base stays at 0%. See `docs/p4-gate-2026-07-04.md`.
