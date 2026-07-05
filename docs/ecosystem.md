# The Claw ecosystem: what exists, what's missing, what wins adoption

The question this answers: what does a language need to feel *prominent* to
developers on launch day — and what does it need for AI companies to adopt
it as an agent target? Benchmark: Rust (the best-tooled young language),
minus what doesn't apply, plus the AI-first layer no incumbent has.

## Inventory vs Rust

| component | Rust | Claw today | gap / next move |
|---|---|---|---|
| Compiler | rustc | `clawc` (Zig, vendored) ✅ | — |
| Installer / updates | rustup | `install.sh` + `claw upgrade` ✅ | first release tag |
| Package manager | cargo + crates.io | `claw publish/add` + registry service (prototype) 🟡 | host the registry (tiny axum/Postgres, or R2-backed) |
| Formatter | rustfmt | `claw fmt` ✅ | — |
| Test runner | cargo test | `claw test` ✅ | wire into grader oracles |
| LSP | rust-analyzer | `claw-lsp` (completion, hover) 🟡 | diagnostics, go-to-def from CDB |
| Editor syntax | everywhere | **none** ❌ | VS Code extension (covers Cursor/Windsurf/VSCodium), then tree-sitter (covers Zed/Neovim/Helix) |
| Playground | play.rust-lang.org | `playground/index.html` (JS mirror of the type/grammar engine) 🟡 | link from site now; hosted real-compile later (server or WASM clawc) |
| Docs site | doc.rust-lang.org + The Book | `docs/*.md` + tour + getting-started 🟡 | website + docs index (site/) |
| Stdlib reference | docs.rs | none ❌ | render from the CDB — `claw db render --docs` is a natural fit |
| CI integration | GitHub Actions | none ❌ | a `setup-claw` action (10 lines around install.sh) |
| Branding | Ferris, logo, consistent voice | name + OG card 🟡 | logo, one-line story, brand page |
| Community | forum, Discord, RFCs | GH Discussions (to enable) ❌ | Discussions + Discord on launch; RFC dir when there are users |
| Debugger | limited | n/a | not launch-relevant |

## The AI-first layer (no incumbent has this — the moat)

| component | status | why an AI company cares |
|---|---|---|
| Code-as-database (CDB) | ✅ | agent asks "what exists?" instead of guessing — hallucination 38→0 measured |
| MCP server (5 tools) | ✅ + docs for 8 clients | drop-in for Claude/Cursor/Gemini/Codex agents today |
| Decode grammar (GBNF per scope) | ✅ | out-of-scope calls *ungeneratable* — a guarantee, not a prompt |
| Real-compile check API (`defs-check`) | ✅ | the verifier loop agents need (arity/type errors caught) |
| Executable contracts | ✅ | semantic verification beyond typecheck |
| Bundled tuned model | ✅ 0.5B (7B measuring now) | 94% functional pass vs 56% stock-Python — the parity story |
| Benchmark harness (5-language parity) | ✅ | reproducible evidence, not vibes |
| Telemetry → training loop | ✅ (worker undeployed) | the model improves from real usage — a data flywheel |
| Agent SDK / cookbook | ❌ | "build a Claw-writing agent in 20 lines" doc — highest-leverage missing piece for AI adoption |
| Hosted eval API | ❌ | let AI labs run the benchmark against THEIR models — that's how adoption starts: as an eval target |

## What actually wins the two audiences

**Developers (launch day):** one-line install → `claw init` → working
program in 60 seconds; an editor that highlights; a playground to try
without installing; a README with numbers not adjectives. Excitement =
"the AI in my editor stops making things up when the project is Claw."

**AI companies:** they adopt *benchmarks and guarantees*, not syntax. The
pitch is: a language where your model's output is verifiable at four
layers, plus a harness proving a $0.03 fine-tune beats Python on
functional correctness. Deliverables that matter: the eval harness as a
product, MCP integration, the grammar API, and a "run your model on the
Claw gate" doc. If two labs publish gate numbers, Claw is a standard.

## Priority order (effort × leverage)

1. **VS Code extension** — hours of work, every demo screenshot needs it
2. **Website + docs index + playground link** — the front door
3. **Registry hosting + first release tag** — makes install real
4. **Agent cookbook + hosted eval doc** — the AI-company wedge
5. **Stdlib reference from CDB, tree-sitter grammar, setup-claw action**
6. Discord/Discussions at launch (community needs a launch to gather around)

Everything above the line is buildable this week at ~$0 infra.
