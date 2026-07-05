# Getting started with Claw

Claw is an AI-agent-first programming language. This page gets you from zero
to a running program in about a minute.

## Install

```sh
curl -fsSL https://clawlang.dev/install.sh | sh
```

This downloads a self-contained toolchain into `~/.claw` and puts `claw` on
your PATH. No system compiler, linker, or network platform is required — the
bundle ships everything: the compiler (with its own linker), the tooling,
and the fine-tuned Claw model with its inference server (`claw ai`).

Check it:

```sh
claw --version
```

## Your first program

```sh
claw new hello
cd hello
claw run
```

`claw new` scaffolds a project:

```
hello/
  main.claw     # your program
  claw.toml     # name, version, entry point
  claw.cdb      # the code-as-database (indexed automatically)
  README.md
```

`claw run` compiles and runs `main.claw`. The starter prints `Hello, world!`.

## The program

```claw
greet = |who| "Hello, ${who}!"

main! = |_args| {
    echo!(greet("world"))
    Ok({})
}
```

- `greet` is a function. `|who| ...` is a lambda; the body is an expression.
- `"${who}"` is string interpolation.
- `main!` is the entry point. The `!` marks it effectful (it can print).
  It receives the command-line arguments as a `List Str` and returns
  `Ok({})` on success.
- `echo!` prints a line.

## Letting the bundled model write Claw for you

Every install includes a fine-tuned model that already speaks Claw. One
command generates a definition — prompted with your project's *real*
symbols, grammar-constrained at decode time, and typechecked by the real
compiler before it's shown:

```sh
claw ai gen "define double : Nat -> Nat"
```

The output prints as `.claw` source followed by a `verified` (real compiler:
OK) or `REJECTED` verdict. Related commands:

```sh
claw ai status   # where the model and server are, and whether it's running
claw ai serve    # start the model server (gen does this automatically)
claw ai stop     # stop it
```

The model (`model/claw-0.5b-q8.gguf`) and inference server (`bin/claw-infer`)
are found automatically inside the install; in a dev checkout, point
`CLAW_MODEL_PATH` and `CLAW_INFER_PATH` at them.

## Letting an AI agent write Claw for you

Claw's headline feature: an agent can't invent APIs that don't exist. Wire
it into Claude Code with one command:

```sh
claw mcp install
```

This registers a local MCP server (`.mcp.json`) that exposes five tools
over *your real code*:

- `claw_symbols` — every function that actually exists, with its type.
- `claw_candidates` — given a target type, which real functions fit.
- `claw_mask` — a decode grammar so out-of-scope calls are ungeneratable.
- `claw_render` — render a Def-JSON definition to `.claw` source.
- `claw_check` — real-compile a definition and get structured errors back.

Re-index after adding files:

```sh
claw index
```

## Packages

The package registry is live at
[registry.clawlang.dev](https://registry.clawlang.dev) (override with
`CLAW_REGISTRY`):

```sh
claw add mylib          # fetch a package, record it in claw.toml
claw publish            # bundle this package and upload it
```

Every published package carries its definitions (names, types, effects,
docs) — the registry rejects a publish without them — and `claw add`
ingests them into your project's `claw.cdb`, so the MCP tools and
`claw ai` know an installed package's API immediately.

## Next

- [The Claw language in 10 minutes](tour.md)
- Runnable examples: [`examples/`](../examples)

## What works today (v0.1)

- **Compile & run** real programs, self-contained.
- **Print + compute + args** with `claw run`.
- **Networking** (a real HTTP server) — `claw new myapi --platform http`
  scaffolds one; see [networking.md](networking.md).
- **AI guardrail** over your real symbol table (via `claw index` + MCP).
- **Bundled model** — `claw ai gen`, grammar-constrained + compiler-verified.
- **Packages** — `claw publish` / `claw add` against the live registry.

See the README's feature matrix for what's experimental vs planned.
