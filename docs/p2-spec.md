# Claw — P2 Deep Spec: Code-as-Database + Generation-Constraint Server

*The make-or-break phase. This is where the thesis lives and where the engineering risk is highest. If P2's gate fails, the project stops here — so this spec is written to be measured, not just built.*

**Org:** LambdaQ Labs · **Lang:** Claw · Fork base: Roc

---

## 0. What P2 must prove

> Binding generation to a real symbol table (code-as-DB) + masking decode to well-typed tokens (constraint server) cuts compile errors **>30%** vs an unconstrained model on the same repo-level benchmark.

Two subsystems, one measurement:
- **WS-B — Code-as-Database (CDB):** the authoritative, queryable model of the program.
- **WS-C — Constraint Server (CS):** turns CDB + type context into a decode-time token mask.

The model never sees raw text as the source of truth; it sees the CDB and generates against the CS.

---

## 1. WS-B — Code-as-Database

### 1.1 Core idea
Source is **not text files**. It's a content-addressed store of definitions (Unison model). Text is a *projection* rendered on demand. This kills three of the top empirical failure modes at once:
- **API hallucination (41% of compile fails)** — you can only reference symbols that exist in the DB.
- **Repo-context collapse (85%→29%)** — cross-file deps are a query, not a reconstruction.
- **Token waste / whole-file reads** — edit one definition by hash, not a file.

### 1.2 Data model

```
Definition
  hash        : blake3(normalized_ast)      # content address, stable identity
  ast         : ClawAST                      # the normalized syntax tree
  type        : TypeScheme                   # inferred, stored
  effects     : EffectRow                    # inferred effect signature
  contract    : Contract?                    # pre/post/invariant (P3; nullable now)
  deps        : [hash]                        # exact definitions this one references
  metadata    : { deprecated: bool, since: version, doc: string }

Name                                         # human/agent-facing, mutable
  name        : "module.func"
  hash        : hash                          # points at a Definition
  # rename = rewrite this row only. Definitions never change identity.
```

Key property: **identity = content hash, name = mutable pointer.** Rename is O(1) metadata. A definition's callers reference it by hash, so renaming never breaks them and never re-parses a file.

### 1.3 Storage
- MVP: **SQLite** — `definitions` table (hash PK, blobs), `names` table, `edges` table (caller_hash, callee_hash). Content blobs in the same DB or a sidecar CAS.
- Dependency graph = the `edges` table. Graph queries = recursive CTEs (fast enough for P2; swap for a real graph store only if it bottlenecks).

### 1.4 Query API (the surface the agent + CS consume)

```
symbols(scope)            -> [Name]              # everything in scope at a point
type_at(cursor)           -> TypeExpectation     # what type is expected here
candidates(type, scope)   -> [Name]              # in-scope defs whose type unifies with `type`
callers(hash)             -> [hash]
deps(hash)                -> [hash]              # transitive available
search(sig)               -> [Name]              # by type signature ("give me a X -> Y")
def(hash)                 -> Definition
render(hash)              -> text                # project to source for human review
```

`candidates(type, scope)` is the heart of it — the CS calls this to know which real symbols could legally appear next.

### 1.5 Edit API (how an agent mutates the program)

```
put(ast)          -> hash          # add/replace a definition; re-infers type+effects+deps
bind(name, hash)  -> ()            # point a name at a hash (create / rename / retarget)
edit(hash, ast')  -> hash'         # new content = new hash; rebind callers if requested
remove(name)      -> ()
```

Agent workflow: fetch `type_at` + `candidates` → generate a single definition → `put` → CDB re-infers → structured errors (WS-D) if it doesn't check → retry. No file touched. No repo re-scanned.

### 1.6 `claw db` CLI

```
claw db symbols <scope>
claw db type-at <file:pos> | <hash:span>
claw db candidates <type> <scope>
claw db callers <name|hash>
claw db search "<sig>"
claw db render <name|hash>
```

---

## 2. WS-C — Generation-Constraint Server

### 2.1 Core idea
The proven lever (PLDI 2025 type-constrained decoding: >50% compile-error reduction). Instead of *rejecting* ill-typed code after generation, make it **ungeneratable**: at each decode step, mask the model's logits to only tokens that can extend a well-typed, in-scope program.

### 2.2 The constraint pipeline

```
model decode step
   │  wants next token
   ▼
Constraint Server
   ├─ parse partial program → find the "hole" (cursor) and its TypeExpectation (via CDB.type_at)
   ├─ compute the set of legal continuations:
   │     · syntactic:  grammar automaton (what tokens the Claw grammar allows here)
   │     · typed:      CDB.candidates(expected_type, scope)  → only real, in-scope, type-fitting symbols
   │     · non-deprecated: filter metadata.deprecated
   │     · (P3) contract-valid: drop symbols whose precondition can't hold in scope
   ├─ project that set to a token mask over the model vocab
   ▼
logits mask → model samples only from legal tokens
```

### 2.3 Protocol (server ↔ decoder)

Request (per step, or batched with speculative lookahead):
```json
{
  "session": "uuid",
  "prefix_tokens": [ ... ],
  "cursor_context": { "scope_hash": "...", "expected_type": "Nat -> Result Ledger Error" }
}
```
Response:
```json
{
  "allowed_token_mask": "<bitset over vocab>",
  "reason": "typed:candidates=[transfer,credit,debit] ∩ grammar",
  "fallback": "if mask empty → widen to grammar-only + flag"
}
```

### 2.4 Integration points
- **vLLM / llama.cpp grammar hooks** — plug the mask in as a logits processor / GBNF-style grammar.
- Keep the CS a **separate process** with a thin RPC — so the same server drives any model backend and is independently testable.
- **Speculative masking:** compute masks for k tokens ahead where the grammar is deterministic (e.g. inside a known call's arg list) to avoid a round-trip per token.

### 2.5 Failure handling
- **Empty mask** (nothing legal): widen to grammar-only, emit a WS-D diagnostic ("no in-scope symbol of type X"), let the agent fetch `search(sig)` or define the missing symbol. Never hard-hang the decoder.
- **Type inference incomplete** mid-expression: fall back to grammar constraint until the expected type is known, then tighten.

---

## 3. How B + C + D compose (one loop)

```
agent: "implement transfer"
  → CDB.type_at / candidates            (B: what's real + expected here)
  → model decodes under CS mask         (C: can only emit legal tokens)
  → CDB.put(ast)                        (B: re-infer type, effects, deps)
  → typecheck
      ok    → done
      fail  → WS-D struct {expected, got, ranked_patches}  (D)
              → agent applies top patch → re-put → re-check
```

Result: API hallucination structurally impossible (C+B), repo context is a query not a guess (B), errors are machine-actionable (D). That is the entire P2 bet, end to end.

---

## 4. P2 Benchmark & Gate

- **Task set (from P0):** ~200 repo-level Claw tasks (translate C/Rust/Py→Claw + from-scratch with real cross-file deps), auto-graded by `claw check` + tests.
- **Arms:**
  1. stock model, no CDB, no CS (baseline)
  2. + CDB context in prompt (no mask)
  3. + CDB + CS mask (full thesis)
- **Metrics:** compile-error rate, iterative Pass@1 (≤3 retries), tokens-to-green.
- **⛔ GATE:** arm 3 must cut compile-error rate **>30%** vs arm 1. Ideally show the API-hallucination class → ~0. If not → STOP or re-architect. Do not proceed to contracts/corpus.

---

## 5. Build order within P2
1. CDB data model + SQLite store + `put`/`type_at`/`candidates` (B minimal).
2. Grammar automaton for Claw (syntactic mask) — get *any* masking working.
3. Typed mask via `candidates` — the real lever.
4. Wire one open model (vLLM) through the CS as a logits processor.
5. Run the 3-arm benchmark. Read the gate.

Everything else in the language waits behind this number.
