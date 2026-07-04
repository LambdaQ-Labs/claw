# Claw CRM — a working backend

A complete CRM backend: customers, deals, a pipeline state machine, and a
summary — running against **real PostgreSQL** with all APIs functioning.

## Architecture (honest)

Claw can't open a database connection or route HTTP yet — there is no such
host (building one is future work). So the backend is split the way Claw is
designed for:

- **`domain.claw`** — the pipeline **rules**, authored in Claw as pure,
  typed logic (a `match` state machine). Runnable and *machine-verified*:
  ```sh
  claw db ingest domain.claw
  claw db eval advance Lead          # -> Qualified
  claw db eval advance Negotiation   # -> Won
  ```
- **`service/`** — a thin Rust service (axum + sqlx) that provides the I/O
  Claw lacks (Postgres + HTTP) and **enforces the exact same transitions**
  the Claw domain specifies and verifies.

The Claw side is the source of truth for the business rules; the Rust side
is the runtime. `advance_stage` in the service mirrors `advance` in the
domain — and `claw db eval` proves the domain's behavior.

## Run

```sh
createdb claw_crm
cd service
DATABASE_URL="postgres://$USER@localhost:5432/claw_crm" cargo run
```

## API

| Method | Path | Does |
|---|---|---|
| GET  | `/health` | liveness |
| POST | `/customers` | create `{name, email}` |
| GET  | `/customers` · `/customers/:id` | list · fetch |
| POST | `/deals` | create `{customer_id, title, value_cents}` (starts `Lead`) |
| GET  | `/deals?stage=` · `/deals/:id` | list/filter · fetch |
| POST | `/deals/:id/advance` | move one stage forward (Claw rules) |
| POST | `/deals/:id/lose` | mark `Lost` |
| GET  | `/pipeline` | count + total value per stage |

Verified end to end: create → advance `Lead→Qualified→Proposal→Negotiation→Won`
→ pipeline summary, with data persisted in Postgres.
