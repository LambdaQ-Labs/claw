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
    // Param pool p0.. matches the Def-JSON output protocol + GBNF grammar.
    let param_names: Vec<String> = (0..params.len()).map(|i| format!("p{i}")).collect();
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

/// A built-in "standard library" scope: a rich set of typed symbols the
/// corpus can synthesize programs over, so a useful corpus exists with no
/// project to ingest. Deterministic.
pub fn stdlib_cdb() -> Cdb {
    use claw_core::{parse::parse_type, Expr, Lit};
    let sigs: &[(&str, &str)] = &[
        ("Nat.add", "Nat, Nat -> Nat"),
        ("Nat.sub", "Nat, Nat -> Nat"),
        ("Nat.mul", "Nat, Nat -> Nat"),
        ("Nat.max", "Nat, Nat -> Nat"),
        ("Nat.min", "Nat, Nat -> Nat"),
        ("Nat.inc", "Nat -> Nat"),
        ("Nat.dec", "Nat -> Nat"),
        ("Nat.double", "Nat -> Nat"),
        ("Nat.isZero", "Nat -> Bool"),
        ("Nat.eq", "Nat, Nat -> Bool"),
        ("Nat.lte", "Nat, Nat -> Bool"),
        ("Nat.toStr", "Nat -> Str"),
        ("Str.concat", "Str, Str -> Str"),
        ("Str.len", "Str -> Nat"),
        ("Str.isEmpty", "Str -> Bool"),
        ("Str.upper", "Str -> Str"),
        ("Bool.and", "Bool, Bool -> Bool"),
        ("Bool.or", "Bool, Bool -> Bool"),
        ("Bool.not", "Bool -> Bool"),
        ("List.len", "List a -> Nat"),
        ("List.isEmpty", "List a -> Bool"),
        ("List.head", "List a -> Maybe a"),
        ("List.reverse", "List a -> List a"),
        ("Maybe.isSome", "Maybe a -> Bool"),
        ("Result.isOk", "Result a e -> Bool"),
    ];
    let mut cdb = Cdb::in_memory().expect("in-memory cdb");
    for (name, sig) in sigs {
        let ty = parse_type(sig).expect("valid stdlib sig");
        let def = Def::new(Expr::Lit(Lit::Str((*name).into())), ty);
        let h = cdb.put(&def).expect("put");
        cdb.bind(name, &h).expect("bind");
    }
    cdb
}

/// Compose examples: for unary `g : A -> B` and unary `f : B -> C`, emit
/// `\p0 -> f (g p0)` : A -> C. Over the stdlib this yields many
/// type-correct, hallucination-free programs.
fn compose_examples(cdb: &Cdb) -> claw_cdb::Result<Vec<Example>> {
    let unary: Vec<(String, Hash, Type, Type)> = cdb
        .symbols()?
        .into_iter()
        .filter_map(|(n, h)| {
            let d = cdb.get(&h).ok()?;
            if let Type::Fn(ps, ret) = &d.ty {
                if ps.len() == 1 {
                    return Some((n, h, ps[0].clone(), (**ret).clone()));
                }
            }
            None
        })
        .collect();

    let mut out = Vec::new();
    for (gname, gh, ga, gb) in &unary {
        for (fname, fh, fb, fc) in &unary {
            // g : ga -> gb ; f : fb -> fc ; composable when gb unifies fb
            if claw_core::unify(gb, fb).is_none() {
                continue;
            }
            let body = Expr::App {
                func: Box::new(Expr::Ref(fh.clone())),
                args: vec![Expr::App {
                    func: Box::new(Expr::Ref(gh.clone())),
                    args: vec![Expr::Var("p0".into())],
                }],
            };
            let ty = Type::Fn(vec![ga.clone()], Box::new(fc.clone()));
            let def = Def::new(
                Expr::Lam {
                    params: vec!["p0".into()],
                    body: Box::new(body),
                },
                ty.clone(),
            );
            let dname = format!(
                "{}_then_{}",
                gname.replace('.', "_").to_lowercase(),
                fname.replace('.', "_").to_lowercase()
            );
            let value = serde_json::json!([{
                "name": dname,
                "expr": def.expr, "ty": def.ty,
                "effects": [], "deprecated": false, "doc": ""
            }]);
            out.push(Example {
                prompt: format!(
                    "Define `{dname}` : {ty} that applies `{gname}` then `{fname}`. Use only in-scope symbols.",
                ),
                completion: serde_json::to_string(&value).unwrap_or_default(),
                uses: vec![gname.clone(), fname.clone()],
            });
        }
    }
    Ok(out)
}

/// Instruction prefixes for prompt augmentation. Same target completion,
/// varied phrasing — teaches the model the output protocol robustly rather
/// than memorizing one instruction style. Standard SFT augmentation.
const PROMPT_PREFIXES: &[&str] = &[
    "",
    "In Claw, ",
    "Write Claw code: ",
    "Task — ",
    "Using only the in-scope symbols, ",
];

/// Multiply examples by re-phrasing each prompt with every prefix.
pub fn augment(examples: &[Example]) -> Vec<Example> {
    let mut out = Vec::with_capacity(examples.len() * PROMPT_PREFIXES.len());
    for ex in examples {
        for pre in PROMPT_PREFIXES {
            let prompt = if pre.is_empty() {
                ex.prompt.clone()
            } else {
                // lowercase the first letter after a prefix for readability
                let mut c = ex.prompt.chars();
                let first = c.next().map(|f| f.to_ascii_lowercase()).unwrap_or_default();
                format!("{pre}{first}{}", c.as_str())
            };
            out.push(Example {
                prompt,
                completion: ex.completion.clone(),
                uses: ex.uses.clone(),
            });
        }
    }
    out
}

/// The full synthetic corpus over the built-in stdlib: (wrappers + compose)
/// × prompt augmentation. This is what `claw corpus gen --stdlib` emits —
/// the training seed for the bundled model.
pub fn generate_stdlib() -> claw_cdb::Result<Vec<Example>> {
    let cdb = stdlib_cdb();
    let mut base = generate(&cdb)?;
    base.extend(compose_examples(&cdb)?);
    Ok(augment(&base))
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
    fn stdlib_corpus_is_large_and_clean() {
        let examples = generate_stdlib().unwrap();
        // (wrappers + compositions) × 5 prompt variants
        assert!(
            examples.len() > 250,
            "expected a sizeable corpus, got {}",
            examples.len()
        );
        // every example references only real stdlib symbols
        let cdb = stdlib_cdb();
        let known: std::collections::BTreeSet<String> =
            cdb.symbols().unwrap().into_iter().map(|(n, _)| n).collect();
        for ex in &examples {
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
