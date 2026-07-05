# Telemetry — opt-in, local-first, near-zero cost

Claw's bundled model was cold-started on synthetic data. The fastest way it
improves is real usage: (prompt, produced definition, compiler verdict)
triples from actual sessions. Telemetry collects exactly that — under three
hard rules.

## The rules

1. **Off by default.** Claw writes NO telemetry unless you set
   `CLAW_TELEMETRY=metrics` or `CLAW_TELEMETRY=full`. There is no silent
   phone-home; uploads only happen when you run `claw telemetry share`.
2. **Local and readable.** Events are plain JSONL at
   `~/.claw/telemetry/events.jsonl` — `cat` it, grep it, delete it.
3. **Bounded.** The log caps at 4 MiB with one rotation (~8 MiB worst
   case). Uploads are one gzipped request.

## Levels

| level | what is recorded |
|---|---|
| *(unset)* | nothing |
| `metrics` | command kind, duration, verdict flags, error counts — no code |
| `full` | also the produced Def-JSON and task prompt — the training-grade signal |

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
