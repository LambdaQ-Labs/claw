#!/usr/bin/env python3
"""P4 gate eval, batched: base-vs-tuned hallucination-free rate on the benchmark.

Same scoring as eval_gate.py, but generates in batches (left-padded greedy)
so a 3090 finishes in minutes instead of ~40. The adapter is toggled with
`disable_adapter()` so base and tuned share one model (the PEFT gotcha).

    python eval_gate_batched.py            # expects ./claw-lora and ../bench/tasks-large
"""
import json, glob, os, torch, re
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import PeftModel

BASE = "Qwen/Qwen2.5-Coder-0.5B-Instruct"
BS = 32
PROTO = open("train.py").read().split('PROTOCOL = """')[1].split('"""')[0]

tok = AutoTokenizer.from_pretrained(BASE, padding_side="left")
if tok.pad_token is None:
    tok.pad_token = tok.eos_token
m = AutoModelForCausalLM.from_pretrained(BASE, torch_dtype=torch.bfloat16, device_map="auto")
m = PeftModel.from_pretrained(m, "claw-lora")  # one model; toggle the adapter


def gen_batch(prompts, tag):
    outs = []
    for i in range(0, len(prompts), BS):
        chunk = prompts[i:i + BS]
        texts = [
            tok.apply_chat_template(
                [{"role": "user", "content": p + "\n\n" + PROTO}],
                tokenize=False, add_generation_prompt=True)
            for p in chunk
        ]
        enc = tok(texts, return_tensors="pt", padding=True).to(m.device)
        out = m.generate(**enc, max_new_tokens=180, do_sample=False,
                         pad_token_id=tok.pad_token_id)
        outs += [tok.decode(out[j][enc.input_ids.shape[1]:], skip_special_tokens=True)
                 for j in range(len(chunk))]
        print(f"{tag}: {len(outs)}/{len(prompts)}", flush=True)
    return outs


def vars_of(x):
    o = []
    if isinstance(x, dict):
        for k, v in x.items():
            o += [v] if (k == "Var" and isinstance(v, str)) else vars_of(v)
    elif isinstance(x, list):
        for v in x:
            o += vars_of(v)
    return o


def expr_vars(defs):
    """Var names from each def's EXPR only — Type::Var serializes as
    {"Var": "a"} too, so walking "ty" misreads generics as references."""
    out = []
    for d in (defs if isinstance(defs, list) else [defs]):
        if isinstance(d, dict):
            out += vars_of(d.get("expr"))
    return out


def check(raw, scope):
    """(valid_json, halluc_free, effects_sound) for one completion."""
    try:
        j = json.loads(raw.strip().strip('`').replace('json', '', 1).strip())
    except Exception:
        return (False, False, False)
    names = set(n for n, _, _ in scope)
    used = [v for v in expr_vars(j) if not re.match(r'^p\d+$', v)]
    hall = [v for v in used if v not in names]
    # Effect soundness: the declared rows must cover the union of the used
    # symbols' rows (mirrors claw_effects::check_by_names in the grader).
    required = set()
    for n, _, eff in scope:
        if n in used:
            required.update(eff)
    declared = set()
    defs = j if isinstance(j, list) else [j]
    for d in defs:
        if isinstance(d, dict):
            declared.update(d.get("effects") or [])
    return (True, len(hall) == 0, required <= declared)


TASKS_DIR = os.environ.get("CLAW_TASKS", "../bench/tasks-large")
files = sorted(glob.glob(TASKS_DIR + "/*.json"))
tasks = [json.load(open(f)) for f in files]
scopes, prompts = [], []
for t in tasks:
    scope = [(s["name"], s["ty"], s.get("effects", [])) for s in t.get("scope", [])]
    scopeln = "\n".join(
        f"  {n} : {s}" + (f"  [effects: {', '.join(e)}]" if e else "")
        for n, s, e in scope)
    scopes.append(scope)
    prompts.append(f"Task: {t['prompt']}\n\nIn-scope symbols (the ONLY callable definitions):\n{scopeln}")

res = {}
with m.disable_adapter():
    res["base"] = gen_batch(prompts, "base")
res["tuned"] = gen_batch(prompts, "tuned")

n = len(tasks)
for k in ("base", "tuned"):
    v = c = e = clean = 0
    for raw, scope in zip(res[k], scopes):
        vi, ci, ei = check(raw, scope)
        v += vi; c += ci; e += ei; clean += (ci and ei)
    print(f"{k}: valid_json={v}/{n} ({100 * v // n}%)  no_halluc={c}/{n} ({100 * c // n}%)  "
          f"effects_sound={e}/{n} ({100 * e // n}%)  clean={clean}/{n} ({100 * clean // n}%)")

# Dump the tuned arm's raw parses for the real-compiler pass back home:
# `claw defs-check --batch outputs.jsonl` (task file path + Def-JSON).
with open("outputs.jsonl", "w") as fh:
    for f, raw in zip(files, res["tuned"]):
        try:
            defs = json.loads(raw.strip().strip('`').replace('json', '', 1).strip())
        except Exception:
            defs = None
        fh.write(json.dumps({"task": f, "defs": defs}) + "\n")
print("wrote outputs.jsonl (tuned arm) for real-compile grading")
