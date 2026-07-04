//! claw-corpus — synthetic training-corpus generator (WS-H).
//!
//! The cold-start problem is the thing that kills new AI-first languages:
//! a language with no code has no training data, so models are worst at
//! exactly the language they'd be used for. Claw's escape is to *make* the
//! data. This crate generates valid `(prompt, Def-JSON)` pairs directly
//! from a CDB — every pair is a real, in-scope, type-correct program the
//! grammar would accept — so a model can be fine-tuned toward the language
//! before any human writes a line of it.
//!
//! Generation here is property-based and self-labeling: we synthesize
//! programs that only reference symbols in the CDB (so they never
//! hallucinate by construction), pair them with a natural-language prompt,
//! and emit JSONL ready for supervised fine-tuning.
//!
//! Spec: master-plan WS-H (the 80% — the cold-start escape).

use claw_cdb::Cdb;
use claw_core::{Def, Expr, Hash, Type};
use serde::{Deserialize, Serialize};

/// One supervised training example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub prompt: String,
    /// The target completion: a JSON array of one produced definition,
    /// in the exact Def-JSON protocol the benchmark runner expects.
    pub completion: String,
    /// Provenance: which in-scope symbols the completion uses. Every one
    /// is real — this corpus never teaches a hallucination.
    pub uses: Vec<String>,
}

/// Generate training examples from a CDB: for every in-scope symbol that
/// is a unary or binary function, synthesize a wrapper definition that
/// applies it, paired with a prompt describing the task. Deterministic.
pub fn generate(cdb: &Cdb) -> claw_cdb::Result<Vec<Example>> {
    let mut out = Vec::new();
    for (name, hash) in cdb.symbols()? {
        let def = cdb.get(&hash)?;
        if let Type::Fn(params, ret) = &def.ty {
            if let Some(ex) = wrapper_example(&name, &hash, params, ret) {
                out.push(ex);
            }
        }
    }
    Ok(out)
}

/// Synthesize `\a0.. -> name a0..` : a point-free wrapper that calls the
/// symbol on fresh params. Always type-correct and hallucination-free.
fn wrapper_example(name: &str, hash: &Hash, params: &[Type], ret: &Type) -> Option<Example> {
    if params.is_empty() || params.len() > 3 {
        return None;
    }
    let param_names: Vec<String> = (0..params.len()).map(|i| format!("a{i}")).collect();
    let body = Expr::App {
        func: Box::new(Expr::Ref(hash.clone())),
        args: param_names.iter().map(|p| Expr::Var(p.clone())).collect(),
    };
    let def = Def::new(
        Expr::Lam {
            params: param_names.clone(),
            body: Box::new(body),
        },
        Type::Fn(params.to_vec(), Box::new(ret.clone())),
    );

    let sig = Type::Fn(params.to_vec(), Box::new(ret.clone()));
    let prompt = format!(
        "Define a function `apply_{}` : {} that forwards its arguments to the in-scope `{}`. \
         Use only in-scope symbols.",
        name.replace('.', "_").to_lowercase(),
        sig,
        name
    );

    // Completion in the named Def-JSON protocol (with the def's own name).
    let value = serde_json::json!([{
        "name": format!("apply_{}", name.replace('.', "_").to_lowercase()),
        "expr": def.expr,
        "ty": def.ty,
        "effects": def.effects,
        "deprecated": false,
        "doc": ""
    }]);

    Some(Example {
        prompt,
        completion: serde_json::to_string(&value).ok()?,
        uses: vec![name.to_string()],
    })
}

/// Serialize examples to JSONL (one JSON object per line) — the standard
/// supervised-fine-tuning input format.
pub fn to_jsonl(examples: &[Example]) -> String {
    examples
        .iter()
        .filter_map(|e| serde_json::to_string(e).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::Lit;

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    fn seed_cdb() -> Cdb {
        let mut cdb = Cdb::in_memory().unwrap();
        // Nat.add : Nat, Nat -> Nat
        let add = Def::new(
            Expr::Lit(Lit::Int(0)),
            Type::Fn(vec![named("Nat"), named("Nat")], Box::new(named("Nat"))),
        );
        let h = cdb.put(&add).unwrap();
        cdb.bind("Nat.add", &h).unwrap();
        // Nat.zero : Nat  (not a function → skipped)
        let z = Def::new(Expr::Lit(Lit::Int(0)), named("Nat"));
        let zh = cdb.put(&z).unwrap();
        cdb.bind("Nat.zero", &zh).unwrap();
        cdb
    }

    #[test]
    fn generates_wrapper_for_functions_only() {
        let cdb = seed_cdb();
        let examples = generate(&cdb).unwrap();
        assert_eq!(examples.len(), 1, "only Nat.add is a function");
        assert_eq!(examples[0].uses, vec!["Nat.add"]);
        assert!(examples[0].prompt.contains("Nat.add"));
    }

    #[test]
    fn completion_is_valid_named_def_json() {
        let cdb = seed_cdb();
        let ex = &generate(&cdb).unwrap()[0];
        // must parse as an array with a name + expr + ty
        let v: serde_json::Value = serde_json::from_str(&ex.completion).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["name"], "apply_nat_add");
        assert!(v[0]["expr"].get("Lam").is_some());
    }

    #[test]
    fn corpus_only_references_real_symbols() {
        // The whole point: no synthesized example teaches a hallucination.
        let cdb = seed_cdb();
        let known: std::collections::BTreeSet<String> =
            cdb.symbols().unwrap().into_iter().map(|(n, _)| n).collect();
        for ex in generate(&cdb).unwrap() {
            for u in &ex.uses {
                assert!(known.contains(u), "corpus used unknown symbol {u}");
            }
        }
    }

    #[test]
    fn jsonl_is_one_object_per_line() {
        let cdb = seed_cdb();
        let jsonl = to_jsonl(&generate(&cdb).unwrap());
        for line in jsonl.lines() {
            let _: Example = serde_json::from_str(line).expect("each line is an Example");
        }
    }
}
