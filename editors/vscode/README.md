# Claw for VS Code

Syntax highlighting, snippets, and editor smarts for
[Claw](https://clawlang.dev) — the AI-first programming language.

Works in **VS Code, Cursor, Windsurf, and VSCodium** (anything that reads
VS Code extensions).

## Features

- Full TextMate grammar for `.claw`: signatures, effectful `!` calls,
  `Module.function` references, tags, string interpolation `${…}`,
  lambdas, match/if, comments
- Snippets: `fn`, `main`, `match`, `if`, `lam`, `module`, `fold`
- Brackets, auto-closing, indentation rules

## The rest of the toolchain

- **Completions & hover** come from `claw-lsp` (ships with the language):
  point your LSP client at `claw-lsp --db path/to/claw.cdb`.
- **AI integration** comes from `claw-mcp` — the MCP server that lets
  Claude/Cursor/Windsurf agents query what actually exists in your
  project and typecheck generated code. See `docs/mcp.md` in the repo.

## Install (until it's on the marketplace)

```sh
cd editors/vscode
npx @vscode/vsce package        # produces claw-lang-0.1.0.vsix
code --install-extension claw-lang-0.1.0.vsix
```

Cursor/Windsurf: same `.vsix`, installed from their extension panes.
