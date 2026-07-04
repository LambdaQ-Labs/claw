# Getting started with Claw

Claw is an AI-agent-first programming language. This page gets you from zero
to a running program in about a minute.

## Install

```sh
curl -fsSL https://clawlang.dev/install.sh | sh
```

This downloads a self-contained toolchain into `~/.claw` and puts `claw` on
your PATH. No system compiler, linker, or network platform is required — the
Claw compiler bundles everything, including its own linker.

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

## Letting an AI agent write Claw for you

Claw's headline feature: an agent can't invent APIs that don't exist. Wire
it into Claude Code with one command:

```sh
claw mcp install
```

This registers a local MCP server (`.mcp.json`) that answers three questions
over *your real code*:

- `claw_symbols` — every function that actually exists, with its type.
- `claw_candidates` — given a target type, which real functions fit.
- `claw_mask` — a decode grammar so out-of-scope calls are ungeneratable.

Re-index after adding files:

```sh
claw index
```

## Next

- [The Claw language in 10 minutes](tour.md)
- Runnable examples: [`examples/`](../examples)

## What works today (v0.1)

- **Compile & run** real programs, self-contained.
- **Print + compute + args** with `claw run`.
- **Networking** (a real HTTP server) via an explicit platform — see
  [networking.md](networking.md). Bundling it as a `claw new` target is next.
- **AI guardrail** over your real symbol table (via `claw index` + MCP).

See the README's feature matrix for what's experimental vs planned.
