# Claw MCP — wire the code-as-database into any AI coding tool

`claw-mcp` is a Model Context Protocol server (stdio transport) over a Claw
CDB. Any MCP client gets five tools:

| tool | what it answers |
|---|---|
| `claw_symbols` | every definition that actually exists (`name : type`) |
| `claw_candidates` | type-directed search: "what in scope has this type?" |
| `claw_mask` | the legal-symbol set + GBNF grammar for constrained decoding |
| `claw_render` | a definition rendered as `.claw` source |
| `claw_check` | typecheck Def-JSON with the REAL compiler (needs `clawc`) |

This is the anti-hallucination loop: an agent asks `claw_candidates` before
writing a call, and verifies with `claw_check` after — instead of inventing
an API and finding out at review time.

It's the same loop the bundled model uses: `claw ai gen` prompts from the
same CDB, constrains decoding with the same grammar `claw_mask` serves, and
verifies with the same real compiler as `claw_check`. MCP hands that loop
to *your* agent. And because `claw add` ingests a package's published defs
into the project CDB, these tools answer over installed packages too.

## Build / locate the binary

```sh
cargo build --release --bin claw-mcp        # → target/release/claw-mcp
```

Point it at your project's CDB (created by `claw index`):

```sh
claw-mcp --db /path/to/project/claw.cdb
```

`claw_check` runs the vendored compiler: put `clawc` on PATH or set
`CLAW_CLAWC=/path/to/clawc` in the server's env.

Below, replace `/abs/path/to/` with your actual paths. Every client speaks
the same stdio protocol — only the config file differs.

## Claude Code

Inside a Claw project, one command does everything (writes `.mcp.json`,
locates `claw-mcp`, indexes the project):

```sh
claw mcp install
```

Or by hand:

```sh
claude mcp add claw -- /abs/path/to/claw-mcp --db /abs/path/to/claw.cdb
```

Or per-project `.mcp.json`:

```json
{
  "mcpServers": {
    "claw": {
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "claw.cdb"],
      "env": { "CLAW_CLAWC": "/abs/path/to/clawc" }
    }
  }
}
```

## Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "claw": {
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "/abs/path/to/claw.cdb"],
      "env": { "CLAW_CLAWC": "/abs/path/to/clawc" }
    }
  }
}
```

## Cursor

`.cursor/mcp.json` in the project (or `~/.cursor/mcp.json` globally):

```json
{
  "mcpServers": {
    "claw": {
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "claw.cdb"],
      "env": { "CLAW_CLAWC": "/abs/path/to/clawc" }
    }
  }
}
```

## Windsurf

`~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "claw": {
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "/abs/path/to/claw.cdb"]
    }
  }
}
```

## VS Code (GitHub Copilot agent mode)

`.vscode/mcp.json`:

```json
{
  "servers": {
    "claw": {
      "type": "stdio",
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "claw.cdb"]
    }
  }
}
```

## Zed

`settings.json` → `context_servers`:

```json
{
  "context_servers": {
    "claw": {
      "source": "custom",
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "/abs/path/to/claw.cdb"]
    }
  }
}
```

## Gemini CLI

`~/.gemini/settings.json`:

```json
{
  "mcpServers": {
    "claw": {
      "command": "/abs/path/to/claw-mcp",
      "args": ["--db", "/abs/path/to/claw.cdb"]
    }
  }
}
```

## Codex CLI

`~/.codex/config.toml`:

```toml
[mcp_servers.claw]
command = "/abs/path/to/claw-mcp"
args = ["--db", "/abs/path/to/claw.cdb"]

[mcp_servers.claw.env]
CLAW_CLAWC = "/abs/path/to/clawc"
```

## Cline / Continue / anything else

Any MCP client that can spawn a stdio server works with the same three
fields: command `claw-mcp`, args `["--db", "<path>"]`, optional env
`CLAW_CLAWC`. There is no HTTP transport yet — file an issue if you need
one.

## Smoke test

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | claw-mcp --db claw.cdb
```

should list the five tools. In your client, ask the agent: *"use
claw_symbols to list what exists, then claw_check this definition"* — if
both round-trip, the loop is closed.
