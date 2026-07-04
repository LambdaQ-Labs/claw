#!/usr/bin/env python3
"""Procedurally generate a large set of Claw benchmark tasks.

Emits scalar-arithmetic tasks with EXECUTABLE contracts (so they can
functionally pass) plus wrapper/compose tasks, into a target dir. Used
both to grow the benchmark and as the prompt set for distillation.

    python bench/gen_tasks.py bench/tasks-large
"""
import json, os, sys, itertools

OUT = sys.argv[1] if len(sys.argv) > 1 else "bench/tasks-large"
os.makedirs(OUT, exist_ok=True)

NAT2 = {  # binary Nat->Nat symbols and a checkable postcondition template
    "Nat.add": ("adds", ["result == a + b", "result >= a"]),
    "Nat.mul": ("multiplies", ["result == a * b"]),
    "Nat.max": ("takes the maximum of", ["result >= a", "result >= b"]),
    "Nat.min": ("takes the minimum of", ["result <= a", "result <= b"]),
}
SCOPE2 = [{"name": n, "ty": "Nat, Nat -> Nat"} for n in NAT2]

def task(tid, prompt, scope, params, requires, contracts):
    return {
        "id": tid, "category": "contract", "prompt": prompt,
        "scope": scope, "params": params,
        "grade": {"compile": True, "requires": requires,
                  "contracts": contracts, "forbidden": ["hallucinated-symbol"]},
    }

n = 0
def write(t):
    global n
    with open(os.path.join(OUT, t["id"] + ".json"), "w") as f:
        json.dump(t, f, indent=2)
    n += 1

# 1) binary arithmetic — each op, params p0,p1 named a,b
for sym, (verb, contracts) in NAT2.items():
    base = sym.replace(".", "_").lower()
    write(task(
        f"gen-{base}",
        f"Define `{base}` : Nat, Nat -> Nat (parameters p0, p1 named a, b) that {verb} its two arguments using in-scope `{sym}`.",
        SCOPE2, [{"name": "a"}, {"name": "b"}], [], contracts))

# 2) clamp family — value clamped into [lo, hi]
for lo_first in (True, False):
    tid = "gen-clamp-" + ("lohi" if lo_first else "hilo")
    write(task(
        tid,
        "Define `clamp` : Nat, Nat, Nat -> Nat (parameters p0, p1, p2 named x, lo, hi) that clamps x into [lo, hi]. Assume lo <= hi. Use only in-scope symbols.",
        SCOPE2, [{"name": "x"}, {"name": "lo"}, {"name": "hi"}],
        ["lo <= hi"], ["result >= lo", "result <= hi"]))

# 3) offset/scale: f(x) = op(x, k) for constant-ish via second param
for sym, (verb, _) in NAT2.items():
    base = sym.replace(".", "_").lower()
    write(task(
        f"gen-{base}-comm",
        f"Define `{base}_comm` : Nat, Nat -> Nat (parameters p0, p1 named a, b) that {verb} the arguments (order may matter) using `{sym}`.",
        SCOPE2, [{"name": "a"}, {"name": "b"}], [], []))

# 4) nested: max(add(a,b), a) style compositions with contracts
combos = [
    ("addmax", "Nat.add", "Nat.max", ["result >= a"]),
    ("mulmin", "Nat.mul", "Nat.min", []),
    ("addmin", "Nat.add", "Nat.min", ["result >= a"]),
]
for name, s1, s2, contracts in combos:
    write(task(
        f"gen-{name}",
        f"Define `{name}` : Nat, Nat -> Nat (parameters p0, p1 named a, b) combining `{s1}` and `{s2}`.",
        SCOPE2, [{"name": "a"}, {"name": "b"}], [], contracts))

# 5) three-arg arithmetic (sum of three, max of three)
SCOPE3 = SCOPE2
for name, verb, contracts in [
    ("sum3", "sums the three arguments", ["result >= a", "result >= b"]),
    ("max3", "returns the maximum of three arguments", ["result >= a", "result >= b", "result >= c"]),
]:
    write(task(
        f"gen-{name}",
        f"Define `{name}` : Nat, Nat, Nat -> Nat (parameters p0, p1, p2 named a, b, c) that {verb} using only in-scope symbols.",
        SCOPE3, [{"name": "a"}, {"name": "b"}, {"name": "c"}], [], contracts))

# 6) unary-with-constant family (the bulk): f(x) = op(x, k). Multiple ops,
#    constants, and prompt phrasings — the diverse volume for distillation.
UNARY = [
    ("add", "Nat.add", "+", ["result == x + {k}", "result >= x"]),
    ("mul", "Nat.mul", "*", ["result == x * {k}"]),
    ("atleast", "Nat.max", "max-with", ["result >= x", "result >= {k}"]),
    ("atmost", "Nat.min", "min-with", ["result <= x", "result <= {k}"]),
]
PHRASINGS = [
    "Define `{name}` : Nat -> Nat (parameter p0 named x) that computes {sym} of x and {k}.",
    "In Claw, define `{name}` (parameter p0 = x) : Nat -> Nat applying `{sym}` to x and the constant {k}.",
    "Write `{name}` : Nat -> Nat using only in-scope `{sym}`; parameter p0 is x, combine it with {k}.",
]
for opname, sym, _desc, ctpl in UNARY:
    for k in (1, 2, 3, 4, 5, 7, 10, 100):
        for pi, phr in enumerate(PHRASINGS):
            tid = f"gen-{opname}{k}-v{pi}"
            contracts = [c.format(k=k) for c in ctpl]
            prompt = phr.format(name=f"{opname}{k}", sym=sym, k=k)
            write(task(tid, prompt, [{"name": sym, "ty": "Nat, Nat -> Nat"}],
                       [{"name": "x"}], [], contracts))

print(f"wrote {n} tasks to {OUT}")
