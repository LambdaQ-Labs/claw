# Bundled-model selection: E4B, Qwen3.5-4B, Qwen2.5-Coder-3B vs the 0.5B — 2026-07-05

The shipped v0.1.0 model is a 0.5B Qwen2.5-Coder fine-tune. It clears the
synthetic gate but its ceiling shows on held-out human-written tasks (76%
clean) and on unusual compositions (the guardrails reject its garbage
rather than shipping it — correct, but a weak model). This experiment
tested whether Google's Gemma 4 E4B (~4.5B effective params, Apache-2.0,
runs CPU-only, LoRA-tunable on our $0.22/hr pod) is worth a v0.2.0 swap.

## Setup

QLoRA (4-bit, r=16) on corpus-v4 (1661 examples, same protocol), 3 epochs,
train loss 0.062, ~24 min on an RTX 3090. Evaluated A1-style (scope in the
prompt, NO grammar constraint) so the comparison is model-quality, not
guardrail-assisted.

## Results (clean = hallucination-free AND effect-sound)

| model | gate 121 · base | gate 121 · tuned | holdout 25 · base | holdout 25 · tuned |
|---|---|---|---|---|
| 0.5B Qwen-Coder (shipped) | ~0% | **100%** | 44% | 76% |
| **Gemma 4 E4B** | **58%** | 95% | **56%** | **88%** |

The headline is the **held-out set** — the honest generalization test,
human-written and off the training distribution:

- 0.5B tuned: 76% clean
- **E4B tuned: 88% clean** — +12 points, and it starts from a 58%-capable
  base vs the 0.5B's ~0%.

On the synthetic gate the 0.5B edges ahead (100% vs 95%), but the gate is
near-ceiling and shares generator DNA with the corpus; it rewards a small
model that memorizes the shapes. The holdout is what a real user's novel
task looks like, and there E4B is clearly stronger.

## The important finding: Gemma 4 is a *reasoning* model

E4B-it emits a `thought\n…` chain-of-reasoning before its answer. First
eval passes scored it 0% because a 200-token budget was entirely consumed
by the thinking trace — no JSON ever appeared. Fair scoring required a
600-token budget and extracting the JSON array from after the reasoning.

This matters for the product: **for grammar-constrained structured output,
a thinking model is a liability** — the reasoning trace fights the GBNF
constraint (you can't constrain prose-then-JSON with one grammar), burns
CPU-bound latency, and complicates the output contract. Under A2 (our
grammar arm) Gemma's thinking would have to be suppressed or the grammar
would have to allow a thinking preamble.

## Recommendation

E4B is the stronger model and the +12-point holdout gain justifies a
v0.2.0 "standard" tier. But two things to resolve first:

1. **Suppress or accommodate the reasoning trace.** Either disable Gemma's
   thinking (system prompt / generation config) for a clean instruct-style
   emit, or teach the corpus/grammar to expect a thinking preamble. Without
   this, the A2 grammar guarantee doesn't compose.
2. **Consider a non-thinking 3-4B coder instead.** A direct instruct/coder
   base (Qwen3-4B in no-think mode, or a Qwen2.5-Coder-3B) may match E4B's
   capability without the reasoning-trace friction — a better fit for a
   structured, grammar-constrained, latency-sensitive local model. Worth a
   one-pod A/B before committing the bundle size.

Proposed v0.2.0 shape (pending the above): ship both tiers via
`--model small|standard`; small = today's 506 MB 0.5B (instant, offline,
weak), standard = a ~2.5 GB 3-4B (stronger, still CPU-viable). Quantize the
winner to GGUF q4_k_m, re-run the full gate + parity + A2 for the launch
headline.

Adapter (`claw-lora-e4b`, 131 MB, gitignored) and raw outputs
(`train/outputs-e4b-{large,holdout}.jsonl`) are kept for the follow-up.
Cost of this experiment: ~$0.30.


---

# Follow-up: the Qwen A/B and a training-path bug (same day)

Per the E4B recommendation, we A/B'd two dense, bundleable, Apache-2.0
alternatives against E4B and the shipped 0.5B. Field note first: **GLM,
MiniMax, and Qwen-4-Coder are all MoE** — small *active* params but the
full expert set (18–200 GB) is what you'd bundle, so they are
un-bundleable and were excluded. The bundleable dense field is Qwen.

## Fair numbers (512-token budget + JSON-array extraction — the same
scoring E4B got; the 0.5B's terser output happened to fit 180 tokens,
which is why an unfair 180-cap scored the bigger models near 0)

| model (holdout 25, A1 no-grammar) | base clean | tuned clean |
|---|---|---|
| 0.5B Qwen-Coder (shipped) | 44% | **76%** |
| Gemma 4 E4B (unsloth QLoRA) | 56% | **88%** |
| Qwen2.5-Coder-3B (our train.py 4-bit) | 56% | **16%** ⚠ |
| Qwen3.5-4B | — | — (blocked) |

Two hard findings:

1. **Strong bases, weak-teaching corpus.** Untrained Coder-3B and E4B both
   score 56% clean on the held-out set with ZERO Claw exposure (vs the
   0.5B base's ~0%). These models already follow the Def-JSON protocol
   in-context. That reframes the whole problem: for a capable base, the
   corpus's job shifts from "teach the format" to "don't degrade it."

2. **Our train.py 4-bit QLoRA path produces broken adapters.** Coder-3B
   tuned collapsed to 16% at BOTH 4 epochs (loss 0.031) and 1 epoch
   (loss 0.155) — epoch-independent, so it is not overfitting. A manual
   generation showed the tuned model emitting near-valid Def-JSON with
   dropped braces (`{"Var": "p0", {"Lit"…`) — an adapter that is applied
   but corrupting output. E4B, trained through **unsloth** QLoRA on the
   same corpus, tuned cleanly to 88%. The variable is the training path,
   not the model: our newly-added `CLAW_4BIT` path in train.py
   (BitsAndBytesConfig + prepare_model_for_kbit_training + gradient
   checkpointing) is suspect; unsloth's path is known-good.

3. **Qwen3.5-4B is blocked on our pinned stack** — its `qwen3_5`
   architecture is unknown to transformers 4.46.3 (the pin trl 0.11.4
   requires). Needs a newer transformers, which means moving the whole
   training stack (or using unsloth, which ships a current one).

## Where this leaves the v0.2.0 base decision

Unsettled — but the path to settle it is now clear and cheap:

- **Re-run Coder-3B and Qwen3.5-4B through the unsloth path** (train_e4b.py
  generalizes; eval_e4b.py handles the tokenizer/template quirks). That
  removes the train.py-4bit confound and unblocks Qwen3.5. One pod, ~$0.60.
- Best VERIFIED tuned result so far is **E4B at 88%**, but it is a
  reasoning model (thought-trace fights the grammar; see above) — a poor
  fit for the A2 constrained path. A non-reasoning coder that tunes as
  cleanly as E4B would be the ideal, and Coder-3B's 56% base suggests it
  can get there once the training path is fixed.
- A second, orthogonal lesson: with 56%-capable bases, **the A2 grammar
  constraint stops being optional** — unconstrained bigger models make
  occasional JSON-structure slips that GBNF would eliminate outright.

Net: keep shipping the 0.5B in v0.1.0. Settle v0.2.0 with one clean
unsloth A/B (Coder-3B vs Qwen3.5-4B) plus an A2 pass, then decide. Total
spend on this exploration: ~$0.70.