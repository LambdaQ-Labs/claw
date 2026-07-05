#!/usr/bin/env python3
"""Merge the LoRA adapter into the base model for GGUF conversion.

llama.cpp consumes a plain HF checkpoint; PEFT adapters must be folded in
first (merge_and_unload). Output: ./merged (model + tokenizer).

    python merge_lora.py            # expects ./claw-lora
"""
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import PeftModel

BASE = "Qwen/Qwen2.5-Coder-0.5B-Instruct"

tok = AutoTokenizer.from_pretrained(BASE)
m = AutoModelForCausalLM.from_pretrained(BASE, torch_dtype=torch.float16)
m = PeftModel.from_pretrained(m, "claw-lora")
m = m.merge_and_unload()
m.save_pretrained("merged")
tok.save_pretrained("merged")
print("merged model at ./merged")
