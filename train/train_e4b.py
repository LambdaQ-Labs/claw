#!/usr/bin/env python3
"""QLoRA fine-tune for the next bundled model candidate (Gemma 4 E4B).

Same corpus, same protocol, bigger brain: the 0.5B's failure mode is
compositional fragility outside the training distribution; a ~4.5B
effective-parameter model is the cheapest credible fix. Unsloth QLoRA
keeps it inside a 24 GB pod.

    python train_e4b.py --model unsloth/gemma-4-e4b-it --corpus corpus-v4.jsonl
"""
import argparse, json

from unsloth import FastLanguageModel
from datasets import Dataset
from trl import SFTConfig, SFTTrainer

PROTOCOL = open("train.py").read().split('PROTOCOL = """')[1].split('"""')[0]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="unsloth/Qwen2.5-Coder-3B-Instruct")
    ap.add_argument("--corpus", default="corpus-v4.jsonl")
    ap.add_argument("--out", default="claw-lora-e4b")
    ap.add_argument("--epochs", type=float, default=3.0)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument("--batch", type=int, default=4)
    args = ap.parse_args()

    model, tok = FastLanguageModel.from_pretrained(
        model_name=args.model,
        max_seq_length=2048,
        load_in_4bit=True,
    )
    model = FastLanguageModel.get_peft_model(
        model,
        r=16,
        lora_alpha=32,
        lora_dropout=0,
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj",
                        "gate_proj", "up_proj", "down_proj"],
    )

    rows = [json.loads(l) for l in open(args.corpus) if l.strip()]
    texts = [
        tok.apply_chat_template(
            [
                {"role": "user", "content": f"{r['prompt']}\n\n{PROTOCOL}"},
                {"role": "assistant", "content": r["completion"]},
            ],
            tokenize=False,
        )
        for r in rows
    ]
    ds = Dataset.from_dict({"text": texts})
    print(f"corpus: {len(ds)} examples")

    trainer = SFTTrainer(
        model=model,
        tokenizer=tok,
        train_dataset=ds,
        args=SFTConfig(
            output_dir=args.out,
            num_train_epochs=args.epochs,
            learning_rate=args.lr,
            per_device_train_batch_size=args.batch,
            gradient_accumulation_steps=4,
            logging_steps=10,
            save_strategy="epoch",
            bf16=True,
            max_seq_length=2048,
            report_to="none",
            dataset_text_field="text",
        ),
    )
    trainer.train()
    model.save_pretrained(args.out)
    tok.save_pretrained(args.out)
    print(f"saved LoRA adapter to {args.out}")


if __name__ == "__main__":
    main()
