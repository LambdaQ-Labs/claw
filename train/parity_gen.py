#!/usr/bin/env python3
"""Generate the parity-eval completions on a GPU pod.

Same 0.5B checkpoint both ways: the TUNED adapter writes Claw (Def-JSON,
A1 prompt), the STOCK base writes Python / JavaScript / Go / Rust for the
same tasks. Downstream, parity_grade.py executes everything against the
same generated test cases — functional Pass@1, apples to apples.

    python parity_gen.py     # expects ./claw-lora, ../bench/tasks-*
"""
import glob, json, os, re, torch
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import PeftModel

BASE = os.environ.get("CLAW_BASE_MODEL", "Qwen/Qwen2.5-Coder-0.5B-Instruct")
BS = int(os.environ.get("CLAW_BS", "24"))
PROTO = open("train.py").read().split('PROTOCOL = """')[1].split('"""')[0]

tok = AutoTokenizer.from_pretrained(BASE, padding_side="left")
if tok.pad_token is None:
    tok.pad_token = tok.eos_token
m = AutoModelForCausalLM.from_pretrained(BASE, torch_dtype=torch.bfloat16, device_map="auto")
m = PeftModel.from_pretrained(m, "claw-lora")

# Nat.* semantics for the non-Claw arms — the same information the Claw
# arm gets from its scope list, phrased for a general-purpose language.
GLOSSARY = ("Here Nat means a non-negative integer. Nat.add(a,b)=a+b, "
            "Nat.mul(a,b)=a*b, Nat.max(a,b)=max, Nat.min(a,b)=min, "
            "Nat.sub(a,b)=a-b but never below 0, Nat.inc(a)=a+1, "
            "Nat.dec(a)=a-1 but never below 0, Nat.double(a)=2a, "
            "Nat.half(a)=a//2, Nat.sqr(a)=a*a, Bool.if(c,x,y)=x if c else y.")

LANGS = {
    "py":  ("Python",     "def {name}({params}):",              "Return ONLY the function definition, no prose, no code fences."),
    "js":  ("JavaScript", "function {name}({params}) {{ ... }}", "Return ONLY the function definition, no prose, no code fences."),
    "go":  ("Go",         "func {name}({goparams}) int64 {{ ... }}", "Return ONLY the function (no package line, no main), no prose, no code fences."),
    "rs":  ("Rust",       "fn {name}({rsparams}) -> i64 {{ ... }}", "Return ONLY the function, no prose, no code fences."),
}


def gen_batch(prompts, tag):
    outs = []
    for i in range(0, len(prompts), BS):
        chunk = prompts[i:i + BS]
        texts = [tok.apply_chat_template([{"role": "user", "content": p}],
                                         tokenize=False, add_generation_prompt=True)
                 for p in chunk]
        enc = tok(texts, return_tensors="pt", padding=True).to(m.device)
        out = m.generate(**enc, max_new_tokens=200, do_sample=False,
                         pad_token_id=tok.pad_token_id)
        outs += [tok.decode(out[j][enc.input_ids.shape[1]:], skip_special_tokens=True)
                 for j in range(len(chunk))]
        print(f"{tag}: {len(outs)}/{len(prompts)}", flush=True)
    return outs


files = sorted(glob.glob("../bench/tasks-large/*.json") + glob.glob("../bench/tasks-holdout/*.json"))
tasks = []
for f in files:
    t = json.load(open(f))
    if t.get("params") and t["grade"].get("contracts"):
        tasks.append((f, t))
print(f"parity tasks: {len(tasks)}")

# --- Claw arm (tuned adapter, A1 prompt) -----------------------------------
claw_prompts = []
for f, t in tasks:
    scope = [(s["name"], s["ty"], s.get("effects", [])) for s in t.get("scope", [])]
    scopeln = "\n".join(f"  {n} : {ty}" + (f"  [effects: {', '.join(e)}]" if e else "")
                        for n, ty, e in scope)
    claw_prompts.append(f"Task: {t['prompt']}\n\nIn-scope symbols (the ONLY callable "
                        f"definitions):\n{scopeln}\n\n{PROTO}")
outs = gen_batch(claw_prompts, "claw-tuned")
with open("parity-claw.jsonl", "w") as fh:
    for (f, _), raw in zip(tasks, outs):
        try:
            defs = json.loads(raw.strip().strip('`').replace('json', '', 1).strip())
        except Exception:
            defs = None
        fh.write(json.dumps({"task": f, "defs": defs, "raw": raw[:400]}) + "\n")

# --- The four stock arms ----------------------------------------------------
def fname(t):
    m_ = re.search(r"`(\w+)`", t["prompt"])
    return m_.group(1) if m_ else "solve"

for lang, (langname, sig, tail) in LANGS.items():
    prompts = []
    for f, t in tasks:
        params = ", ".join(p["name"] for p in t["params"])
        goparams = ", ".join(f"{p['name']} int64" for p in t["params"])
        rsparams = ", ".join(f"{p['name']}: i64" for p in t["params"])
        signature = sig.format(name=fname(t), params=params, goparams=goparams, rsparams=rsparams)
        prompts.append(
            f"Task: {t['prompt']}\n\n{GLOSSARY}\n\nWrite this in {langname} with the "
            f"signature `{signature}`. {tail}")
    with m.disable_adapter():
        outs = gen_batch(prompts, f"stock-{lang}")
    with open(f"parity-{lang}.jsonl", "w") as fh:
        for (f, t), raw in zip(tasks, outs):
            fh.write(json.dumps({"task": f, "name": fname(t), "raw": raw}) + "\n")

print("PARITY-GEN-DONE")
