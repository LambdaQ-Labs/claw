# Telemetry — anonymous metrics by default, one command to turn off

Claw's bundled model was cold-started on synthetic data. The fastest way it
improves is real usage: (prompt, produced definition, compiler verdict)
triples from actual sessions. Telemetry collects exactly that — under three
hard rules.

## The rules

1. **Metrics only, and it says so.** By default claw records command
   kinds, verdict flags, and error counts — never your source code, file
   paths, or prompts. The first recorded event prints a notice with the
   off switch. Uploads happen automatically once the local log crosses
   ~64 KiB (one gzipped request; failures are silent and retried later).
2. **One command to stop.** `claw telemetry off` persists your choice;
   `CLAW_TELEMETRY=off` does the same per-environment and wins over the
   file. Off means zero writes, not "collected but not sent".
3. **Local and readable.** Events are plain JSONL at
   `~/.claw/telemetry/events.jsonl` — cat it, grep it, delete it. Capped
   at 4 MiB with one rotation.
4. **Code sharing stays opt-in.** The `full` level (produced Def-JSON +
   prompts — the training-grade signal) is only ever enabled explicitly:
   `claw telemetry full`.

## Levels

| level | what is recorded |
|---|---|
| `metrics` *(default)* | command kind, verdict flags, error counts — no code |
| `off` | nothing — zero writes |
| `full` *(explicit opt-in)* | also the produced Def-JSON and task prompt — the training-grade signal |

Currently instrumented: `claw defs-check` (single-task mode) and the MCP
`claw_check` tool — the two places a model's output meets the real
compiler.

## Commands

```sh
claw telemetry            # status: level, log size, event count
claw telemetry share      # gzip + upload, clear local log on success
claw telemetry clear      # delete the local log
```

`CLAW_TELEMETRY_URL` overrides the ingest endpoint (default: the deployed
worker at `claw-telemetry.ninad2471.workers.dev`; moves to
`telemetry.clawlang.dev` when the domain routes).

## Server side (why this costs ~nothing)

`telemetry/worker/` is a Cloudflare Worker that writes each upload to R2 at
`v1/<date>/<uuid>.jsonl.gz`. Free tiers: 100k requests/day (Workers), 10 GB
+ 1M writes/month (R2). One upload per user per session at a few KiB means
**$0/month until there are thousands of active users** — and R2 charges no
egress when training runs pull the data.

**Deployed 2026-07-05** — bucket `claw-telemetry` (APAC), worker
`claw-telemetry` at `claw-telemetry.ninad2471.workers.dev`, verified
end-to-end (events → gzip → 200 → R2; worker tail shows outcome:ok, and
the gzip round-trips through standard decoders). Remaining: route
`telemetry.clawlang.dev` to the worker when DNS moves to Cloudflare.

```sh
cd telemetry/worker      # to redeploy after changes
npx wrangler deploy
```

## From telemetry to training data

Each `full`-level event carries `{prompt, defs}` + the verdict — the same
schema as the synthetic corpus, so the training pipeline consumes it
directly: verified-good events become SFT pairs (`claw corpus` shapes),
failed ones become contrastive/repair examples. Nothing else to build.
