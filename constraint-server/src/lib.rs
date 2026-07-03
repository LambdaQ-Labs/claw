//! claw-constraint — the generation-constraint core (WS-C).
//!
//! Given a typed hole (cursor + expected type), compute the set of legal
//! continuations: real, in-scope, non-deprecated definitions whose types
//! unify with what the hole expects. The decode-time integration (vLLM /
//! llama.cpp logits mask) projects this set onto the model's vocabulary;
//! this crate is the source of truth that mask is built from.
//!
//! The guarantee this encodes: **a symbol that is not in the CDB cannot
//! appear in the continuation set** — API hallucination is structurally
//! impossible, not post-hoc detected.
//!
//! Spec: docs/p2-spec.md §2.

pub mod gbnf;

use claw_cdb::{Cdb, Result};
use claw_core::{Hash, Subst, Type};
use claw_diagnostics::{Category, Diagnostic, Loc};
use serde::{Deserialize, Serialize};

/// A typed hole: where generation is happening and what must go there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoleContext {
    /// The definition being edited (its current content hash), if any.
    pub editing: Option<Hash>,
    /// The type the hole must produce.
    pub expected: Type,
}

/// One legal continuation: a real symbol the model may emit here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Continuation {
    pub name: String,
    pub hash: Hash,
    pub ty: Type,
    /// What the query's type variables resolved to for this candidate —
    /// tells the decoder what the expression's type becomes if chosen.
    pub subst: Subst,
}

/// The constraint result handed to the decoder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Mask {
    /// Emit only these symbols (projected to tokens by the decoder layer).
    Symbols(Vec<Continuation>),
    /// Nothing in scope fits: decoder falls back to grammar-only and the
    /// agent receives the attached diagnostic (define the missing symbol,
    /// widen the search, or import).
    EmptyWithDiagnostic(Diagnostic),
}

impl Mask {
    /// Project this mask to a llama.cpp GBNF grammar over the Def-JSON
    /// output protocol. An empty mask still yields a valid grammar
    /// (params-only); its diagnostic tells the agent why nothing fit.
    pub fn to_gbnf(&self) -> String {
        match self {
            Mask::Symbols(list) => gbnf::def_json_grammar(list),
            Mask::EmptyWithDiagnostic(_) => gbnf::def_json_grammar(&[]),
        }
    }
}

/// The core function: legal continuations for a typed hole.
/// Filters deprecated symbols — the model should never be steered into
/// APIs we already know are on the way out.
pub fn legal_continuations(cdb: &Cdb, hole: &HoleContext) -> Result<Mask> {
    let mut list: Vec<Continuation> = cdb
        .candidates(&hole.expected)?
        .into_iter()
        .filter(|c| !c.deprecated)
        .map(|c| Continuation {
            name: c.name,
            hash: c.hash,
            ty: c.ty,
            subst: c.subst,
        })
        .collect();
    // Deterministic order: decoder masks must be reproducible run-to-run.
    list.sort_by(|a, b| a.name.cmp(&b.name));

    if list.is_empty() {
        let loc = Loc {
            hash: hole.editing.clone().unwrap_or(Hash(String::from("hole"))),
            span: (0, 0),
        };
        return Ok(Mask::EmptyWithDiagnostic(Diagnostic {
            loc,
            code: "E-SCOPE-0001".into(),
            category: Category::UnknownSymbol,
            expected: Some(hole.expected.to_string()),
            got: None,
            minimal_constraint: format!(
                "no in-scope symbol has type `{}`; define one or widen the query",
                hole.expected
            ),
            patches: vec![],
        }));
    }
    Ok(Mask::Symbols(list))
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::{Def, Expr, Lit};

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    fn sub_ty() -> Type {
        Type::Fn(
            vec![named("Nat"), named("Nat")],
            Box::new(Type::App(
                "Result".into(),
                vec![named("Nat"), named("MathErr")],
            )),
        )
    }

    fn setup() -> Cdb {
        let mut db = Cdb::in_memory().unwrap();

        // Nat.checkedSub : Nat, Nat -> Result Nat MathErr
        let sub = Def::new(Expr::Lit(Lit::Int(0)), sub_ty());
        let h = db.put(&sub).unwrap();
        db.bind("Nat.checkedSub", &h).unwrap();

        // Nat.oldSub — same type, deprecated
        let mut old = Def::new(Expr::Lit(Lit::Int(1)), sub_ty());
        old.deprecated = true;
        let h2 = db.put(&old).unwrap();
        db.bind("Nat.oldSub", &h2).unwrap();

        db
    }

    #[test]
    fn only_real_symbols_appear() {
        let db = setup();
        let hole = HoleContext {
            editing: None,
            expected: Type::Fn(
                vec![named("Nat"), named("Nat")],
                Box::new(Type::Var("a".into())),
            ),
        };
        match legal_continuations(&db, &hole).unwrap() {
            Mask::Symbols(list) => {
                // `generate_nonce`-style hallucinations are impossible by
                // construction: the mask only contains what the CDB holds.
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].name, "Nat.checkedSub");
            }
            Mask::EmptyWithDiagnostic(_) => panic!("expected symbols"),
        }
    }

    #[test]
    fn deprecated_symbols_are_masked_out() {
        let db = setup();
        let hole = HoleContext {
            editing: None,
            expected: sub_ty(),
        };
        match legal_continuations(&db, &hole).unwrap() {
            Mask::Symbols(list) => {
                assert!(list.iter().all(|c| c.name != "Nat.oldSub"));
            }
            Mask::EmptyWithDiagnostic(_) => panic!("expected symbols"),
        }
    }

    #[test]
    fn empty_scope_yields_actionable_diagnostic_not_hang() {
        let db = setup();
        let hole = HoleContext {
            editing: None,
            expected: Type::Fn(vec![named("Ghost")], Box::new(named("Ghost"))),
        };
        match legal_continuations(&db, &hole).unwrap() {
            Mask::EmptyWithDiagnostic(d) => {
                assert_eq!(d.category, Category::UnknownSymbol);
                assert!(d.minimal_constraint.contains("no in-scope symbol"));
            }
            Mask::Symbols(_) => panic!("expected empty mask"),
        }
    }

    #[test]
    fn mask_order_is_deterministic() {
        let mut db = setup();
        // add a second matching symbol
        let add = Def::new(
            Expr::Lit(Lit::Int(2)),
            Type::Fn(vec![named("Nat"), named("Nat")], Box::new(named("Nat"))),
        );
        let h = db.put(&add).unwrap();
        db.bind("Nat.add", &h).unwrap();

        let hole = HoleContext {
            editing: None,
            expected: Type::Fn(
                vec![named("Nat"), named("Nat")],
                Box::new(Type::Var("a".into())),
            ),
        };
        for _ in 0..3 {
            match legal_continuations(&db, &hole).unwrap() {
                Mask::Symbols(list) => {
                    let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
                    assert_eq!(names, vec!["Nat.add", "Nat.checkedSub"]);
                }
                _ => panic!(),
            }
        }
    }
}
