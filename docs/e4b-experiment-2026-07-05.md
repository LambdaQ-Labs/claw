# Experiment: Gemma 4 E4B as the next bundled model — 2026-07-05

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
