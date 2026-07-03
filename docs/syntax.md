# Claw — Syntax Sketch

*Illustrative, not final. Goal: show how contracts, effects/capabilities, code-as-database editing, and structured errors feel in real `.claw` programs. Surface derives from Roc (whitespace-light, ML-family, no borrow checker). File ext `.claw`, CLI `claw`.*

---

## 1. Hello + basics

```claw
# module declaration
module hello

# top-level def. Types inferred but storable/printable.
greet = \name -> "Hello, $(name)!"

main = \{} ->
    Stdout.line! (greet "world")
```

- `\x -> ...` lambda (Roc-style). `!` marks an effectful call (desugars to the effect system).
- `$(...)` string interpolation.

---

## 2. Contracts — the "compiles but wrong" defense (WS-E)

Contracts sit *next to* the impl. `requires`/`ensures`/`example`. Machine-checked where decidable, property-tested otherwise.

```claw
transfer : Account, Account, Nat -> Result Ledger TransferErr
  requires amt <= from.balance                      # precondition
  ensures  ok(result) => from'.balance == from.balance - amt   # postcondition (' = post-state)
  ensures  ok(result) => to'.balance   == to.balance   + amt
  example  transfer (acc 100) (acc 0) 30 == Ok (ledger 70 30)
transfer = \from, to, amt ->
    when Nat.checkedSub from.balance amt is
        Err _   -> Err InsufficientFunds
        Ok left -> Ok (Ledger.of { from: left, to: to.balance + amt })
```

If the body could violate a postcondition, `claw check` fails *before* runtime — catching intent misalignment, not just type errors.

---

## 3. Effects & capabilities — visible blast radius (WS-F)

Effects are in the signature. Nothing does I/O without a **capability** passed in. Sharpened from Roc platforms.

```claw
# The [Read, Net] effect row is inferred and shown. `cap:` = required capabilities.
fetchUser : UserId -> Task User [Net, Read]
  with cap: { http: HttpGet, db: DbRead }
fetchUser = \id ->
    cached = db.get! id            # uses cap.db, contributes [Read]
    when cached is
        Ok u  -> Task.ok u
        Err _ ->
            u = http.get! "/users/$(id)"   # uses cap.http, contributes [Net]
            db.put! id u
            Task.ok u
```

- An agent (and the sandbox) can read the signature and know *exactly* what this touches: `[Net, Read]`, caps `http` + `db`. Nothing hidden.
- A pure function has an empty effect row `[]` — statically guaranteed no side effects. Safe to run/parallelize/memoize.

Running under a restricted capability set = the autonomous-agent sandbox:

```claw
# grant only DB read; no network. fetchUser won't type-check to run here.
runSandboxed : Task a effects -> Result a CapErr
  where effects <= [Read]        # compile error if the task needs Net
```

---

## 4. Code-as-database editing — agent edits by hash, not file (WS-B)

The agent doesn't open files. It queries and patches definitions.

```
# what's expected + available at this hole?
$ claw db type-at transfer:body
  expected: Result Ledger TransferErr
$ claw db candidates "Nat -> Nat -> Result Nat _" scope=transfer
  Nat.checkedSub   (#a1f3)   Nat, Nat -> Result Nat MathErr
  Nat.saturatingSub(#0c9e)   Nat, Nat -> Nat            # note: no error channel

# agent picks the real symbol #a1f3 (can't hallucinate one that isn't listed)
$ claw db put --def transfer <ast>
  #7b2e  transfer : Account, Account, Nat -> Result Ledger TransferErr   ✓ checks

# rename is O(1) metadata — callers reference #7b2e, not the name
$ claw db bind Ledger.transfer #7b2e
```

Because `candidates` only returns real, in-scope, type-fitting symbols, the model **cannot emit `generate_nonce()`** if no such definition exists — API hallucination is structurally impossible, not merely caught.

---

## 5. Structured errors — machine-actionable (WS-D)

Prose is a rendering; the struct is the source of truth the agent consumes.

```
$ claw check transfer
```
```json
{
  "loc": { "hash": "#7b2e", "span": [4, 12] },
  "code": "E-TYPE-0007",
  "category": "type_mismatch",
  "expected": "Result Ledger TransferErr",
  "got": "Ledger",
  "minimal_constraint": "branch must return Result; wrap in Ok",
  "patches": [
    { "rank": 1, "edit": "wrap `Ledger.of {...}` in `Ok (...)`" },
    { "rank": 2, "edit": "change return type to `Ledger`" }
  ],
  "render": "This branch returns `Ledger` but `transfer` promises `Result Ledger TransferErr`. Wrap it in `Ok`."
}
```

Agent reads `patches[0]`, applies, re-`put`s, re-checks. No prose parsing.

---

## 6. Interop — inherit the Rust ecosystem (WS-G)

```claw
# call a real Rust crate through FFI; effects + types declared at the boundary
extern rust "sha2" {
    sha256 : Bytes -> Bytes  [] # pure
}

digest = \data -> sha256 data
```

And any Claw module compiles out via `claw build --emit=rust`, so the outside world consumes Claw as a normal Rust dependency. No "roll your own auth" ecosystem death.

---

## 7. Putting it together — a tiny agent-authored module

```claw
module wallet

Account : { id: UserId, balance: Nat }

transfer : Account, Account, Nat -> Result Ledger TransferErr
  requires amt <= from.balance
  ensures  ok(result) => from'.balance == from.balance - amt
transfer = \from, to, amt ->
    when Nat.checkedSub from.balance amt is
        Err _   -> Err InsufficientFunds
        Ok left -> Ok (Ledger.of { from: left, to: to.balance + amt })

# effectful entry: needs a DB write capability, declared + visible
commit : Ledger -> Task {} [Write]
  with cap: { db: DbWrite }
commit = \l -> db.write! l
```

Every property the research demanded is visible here: no borrow-checker noise, effects/caps explicit, contracts inline, symbols real (from the DB), errors machine-actionable.
