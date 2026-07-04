//! claw-effects — effect inference and capability checking (WS-F).
//!
//! Every definition has an *effect row*: the set of effects (`Net`,
//! `Read`, `Write`, …) it may perform. A pure function's row is empty —
//! statically safe to memoize, reorder, or run in a sandbox. Effects are
//! inferred bottom-up: a def's row is the union of the rows of every
//! definition it references (transitively via the CDB), plus its own.
//!
//! This gives an agent (and the runtime sandbox) the exact blast radius of
//! any code before running it, and lets us reject code that performs an
//! effect it didn't declare or that no capability grants.
//!
//! Spec: docs/syntax.md §3, master-plan WS-F.

use claw_cdb::Cdb;
use claw_core::{Def, Expr, Hash};
use std::collections::BTreeSet;

/// An effect row: a sorted set of effect labels. Empty = pure.
pub type EffectRow = BTreeSet<String>;

/// The capability each effect requires to run. `Net` needs a net cap, etc.
/// (Prototype: identity mapping effect→capability name.)
pub fn required_capability(effect: &str) -> String {
    effect.to_string()
}

/// Infer the effect row of a definition, unioning the declared effects of
/// everything it transitively references in the CDB. Cycle-safe.
pub fn infer(cdb: &Cdb, def: &Def) -> claw_cdb::Result<EffectRow> {
    let mut row: EffectRow = def.effects.iter().cloned().collect();
    let mut seen: BTreeSet<Hash> = BTreeSet::new();
    let mut stack: Vec<Hash> = collect_refs(&def.expr);
    while let Some(h) = stack.pop() {
        if !seen.insert(h.clone()) {
            continue;
        }
        if let Ok(d) = cdb.get(&h) {
            row.extend(d.effects.iter().cloned());
            stack.extend(collect_refs(&d.expr));
        }
    }
    Ok(row)
}

fn collect_refs(e: &Expr) -> Vec<Hash> {
    e.refs()
}

/// Infer effects for code that references scope symbols by *name* (as the
/// benchmark's Def-JSON does: `{"Var": "Store.put"}`), rather than by hash.
/// For every free variable that resolves to a bound CDB symbol, union that
/// symbol's declared effects. This is how we check effect-soundness of
/// model-produced code, whose references are names, not content hashes.
pub fn infer_by_names(cdb: &Cdb, def: &Def) -> claw_cdb::Result<EffectRow> {
    let mut row: EffectRow = def.effects.iter().cloned().collect();
    for name in def.expr.free_vars() {
        if let Ok(h) = cdb.resolve(&name) {
            if let Ok(d) = cdb.get(&h) {
                row.extend(d.effects.iter().cloned());
            }
        }
    }
    Ok(row)
}

/// Name-based soundness check (declared effects must cover name-inferred).
pub fn check_by_names(cdb: &Cdb, def: &Def) -> claw_cdb::Result<EffectCheck> {
    let declared: EffectRow = def.effects.iter().cloned().collect();
    let inferred = infer_by_names(cdb, def)?;
    let undeclared: EffectRow = inferred.difference(&declared).cloned().collect();
    Ok(EffectCheck {
        inferred,
        declared,
        undeclared,
    })
}

/// Result of checking a definition's declared effects against inference.
#[derive(Debug, PartialEq)]
pub struct EffectCheck {
    pub inferred: EffectRow,
    pub declared: EffectRow,
    /// Effects performed but not declared — an unsound signature.
    pub undeclared: EffectRow,
}

impl EffectCheck {
    pub fn is_sound(&self) -> bool {
        self.undeclared.is_empty()
    }
}

/// Check that a def's declared effect row covers everything it actually
/// does. `undeclared` non-empty ⇒ the signature under-claims its effects.
pub fn check(cdb: &Cdb, def: &Def) -> claw_cdb::Result<EffectCheck> {
    let declared: EffectRow = def.effects.iter().cloned().collect();
    let inferred = infer(cdb, def)?;
    let undeclared: EffectRow = inferred.difference(&declared).cloned().collect();
    Ok(EffectCheck {
        inferred,
        declared,
        undeclared,
    })
}

/// Can this definition run under the given capability set? Every inferred
/// effect must have its required capability granted. Returns the missing
/// capabilities (empty = runnable).
pub fn missing_capabilities(
    cdb: &Cdb,
    def: &Def,
    granted: &BTreeSet<String>,
) -> claw_cdb::Result<BTreeSet<String>> {
    let row = infer(cdb, def)?;
    Ok(row
        .iter()
        .map(|e| required_capability(e))
        .filter(|cap| !granted.contains(cap))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::{Lit, Type};

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    fn effectful(effects: &[&str]) -> Def {
        let mut d = Def::new(Expr::Lit(Lit::Int(0)), named("Task"));
        d.effects = effects.iter().map(|s| s.to_string()).collect();
        d
    }

    #[test]
    fn pure_function_has_empty_row() {
        let cdb = Cdb::in_memory().unwrap();
        let d = Def::new(Expr::Lit(Lit::Int(1)), named("Nat"));
        assert!(infer(&cdb, &d).unwrap().is_empty());
    }

    #[test]
    fn effects_propagate_through_references() {
        let mut cdb = Cdb::in_memory().unwrap();
        // db.write! : [Write]
        let writer = effectful(&["Write"]);
        let wh = cdb.put(&writer).unwrap();
        cdb.bind("db.write", &wh).unwrap();

        // http.get! : [Net]
        let netter = effectful(&["Net"]);
        let nh = cdb.put(&netter).unwrap();
        cdb.bind("http.get", &nh).unwrap();

        // caller references both → row = {Net, Write}
        let caller = Def::new(
            Expr::App {
                func: Box::new(Expr::Ref(wh)),
                args: vec![Expr::Ref(nh)],
            },
            named("Task"),
        );
        let row = infer(&cdb, &caller).unwrap();
        assert!(row.contains("Write") && row.contains("Net") && row.len() == 2);
    }

    #[test]
    fn undeclared_effect_is_unsound() {
        let mut cdb = Cdb::in_memory().unwrap();
        let writer = effectful(&["Write"]);
        let wh = cdb.put(&writer).unwrap();

        // caller uses a Write effect but declares itself pure
        let caller = Def::new(Expr::Ref(wh), named("Task")); // no declared effects
        let chk = check(&cdb, &caller).unwrap();
        assert!(!chk.is_sound());
        assert!(chk.undeclared.contains("Write"));
    }

    #[test]
    fn declared_effects_cover_inference() {
        let mut cdb = Cdb::in_memory().unwrap();
        let writer = effectful(&["Write"]);
        let wh = cdb.put(&writer).unwrap();
        let mut caller = Def::new(Expr::Ref(wh), named("Task"));
        caller.effects = vec!["Write".into()];
        assert!(check(&cdb, &caller).unwrap().is_sound());
    }

    #[test]
    fn sandbox_rejects_ungranted_effect() {
        let mut cdb = Cdb::in_memory().unwrap();
        let netter = effectful(&["Net"]);
        let nh = cdb.put(&netter).unwrap();
        let caller = Def::new(Expr::Ref(nh), named("Task"));

        // grant only Read → Net is missing (network sandbox violation)
        let granted: BTreeSet<String> = ["Read".to_string()].into_iter().collect();
        let missing = missing_capabilities(&cdb, &caller, &granted).unwrap();
        assert!(missing.contains("Net"));

        // grant Net → runnable
        let granted2: BTreeSet<String> = ["Net".to_string()].into_iter().collect();
        assert!(missing_capabilities(&cdb, &caller, &granted2)
            .unwrap()
            .is_empty());
    }
}
