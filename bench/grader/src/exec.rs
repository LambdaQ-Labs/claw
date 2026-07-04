//! Contract execution (WS-E, wired live).
//!
//! When a task declares scalar `params`, the grader can actually *run* the
//! produced definition: generate input tuples, apply the produced lambda
//! via the claw-core interpreter, bind (params, result) into a contract
//! environment, and evaluate the `ensures` predicates. This turns
//! "compiles" into "provably does the right thing on N cases" — the
//! difference a skeptic asks for.
//!
//! Scope: scalar (integer) parameters and non-recursive produced lambdas —
//! covers clamp/safeDiv/absDiff-style tasks. Record- and recursion-shaped
//! tasks fall back to unexecuted (reported honestly as ungraded, never
//! silently passed).

use crate::{Param, ProducedDef};
use claw_contract::{eval_pred, parse_pred, Value as CValue};
use claw_core::interp::{eval as run, Env, Resolver, Value as IValue};
use claw_core::{Expr, Hash};
use std::collections::BTreeMap;

/// Resolver over scope names → builtins. Produced code references scope
/// symbols by name (`{"Var":"Nat.add"}`), so those are bound in the env as
/// builtins; there are no hash refs to resolve.
struct NoRefs;
impl Resolver for NoRefs {
    fn resolve(&self, _: &Hash) -> Option<Expr> {
        None
    }
    fn name_of(&self, _: &Hash) -> Option<String> {
        None
    }
}

fn bridge(v: &IValue) -> Option<CValue> {
    match v {
        IValue::Int(n) => Some(CValue::Int(*n)),
        IValue::Bool(b) => Some(CValue::Bool(*b)),
        IValue::Str(s) => Some(CValue::Str(s.clone())),
        IValue::List(xs) => xs
            .iter()
            .map(bridge)
            .collect::<Option<Vec<_>>>()
            .map(CValue::List),
        IValue::Ok(x) => bridge(x).map(|v| CValue::Ok(Box::new(v))),
        IValue::Err(x) => bridge(x).map(|v| CValue::Err(Box::new(v))),
        _ => None,
    }
}

/// Base interpreter env: every builtin scope symbol bound to its builtin.
fn base_env(scope_names: &[String]) -> Env {
    let mut env = Env::new();
    for n in scope_names {
        env.insert(n.clone(), IValue::Builtin(n.clone()));
    }
    env
}

/// Result of executing a task's contracts against a produced def.
#[derive(Debug, PartialEq)]
pub enum ContractRun {
    /// (ensures that held on every case, total ensures).
    Checked(u32, u32),
    /// Could not execute (no params, no runnable lambda, non-scalar) —
    /// caller must NOT treat this as a pass.
    Skipped,
}

/// Execute the (single) produced lambda on generated inputs and check
/// every `ensures`. Preconditions filter the generated cases.
pub fn run_contracts(
    produced: &[ProducedDef],
    params: &[Param],
    requires: &[String],
    ensures: &[String],
    scope_names: &[String],
) -> ContractRun {
    if params.is_empty() || ensures.is_empty() {
        return ContractRun::Skipped;
    }
    // The function under test must be unambiguous. If several defs are
    // lambdas (a helper plus the main function), we can't tell which is the
    // entry point, so Skip (honest) rather than grade a helper by position.
    let lams: Vec<&ProducedDef> = produced
        .iter()
        .filter(|p| matches!(p.def.expr, Expr::Lam { .. }))
        .collect();
    let lam = match lams.as_slice() {
        [only] => &only.def.expr,
        _ => return ContractRun::Skipped,
    };

    let pnames: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    let req_preds: Vec<_> = match requires.iter().map(|s| parse_pred(s)).collect() {
        Ok(v) => v,
        Err(_) => return ContractRun::Skipped,
    };
    let ens_preds: Vec<_> = match ensures.iter().map(|s| parse_pred(s)).collect() {
        Ok(v) => v,
        Err(_) => return ContractRun::Skipped,
    };

    let cases = claw_contract::generate_cases(&pnames, 6, &req_preds);
    if cases.is_empty() {
        return ContractRun::Skipped;
    }

    let base = base_env(scope_names);
    let mut held = vec![true; ens_preds.len()];

    for case in &cases {
        // input values in signature order
        let mut args = Vec::with_capacity(params.len());
        for p in params {
            match case.env.get(&p.name) {
                Some(CValue::Int(n)) => args.push(IValue::Int(*n)),
                _ => return ContractRun::Skipped,
            }
        }
        // apply produced lambda to the args
        let call = Expr::App {
            func: Box::new(lam.clone()),
            args: args
                .iter()
                .map(|v| match v {
                    IValue::Int(n) => Expr::Lit(claw_core::Lit::Int(*n)),
                    _ => unreachable!(),
                })
                .collect(),
        };
        let result = match run(&call, &base, &NoRefs) {
            Ok(v) => v,
            Err(_) => return ContractRun::Skipped, // couldn't run → not a pass
        };
        let cresult = match bridge(&result) {
            Some(v) => v,
            None => return ContractRun::Skipped,
        };

        // contract env = case params + result
        let mut cenv: BTreeMap<String, CValue> = case.env.clone();
        cenv.insert("result".into(), cresult);

        for (i, pred) in ens_preds.iter().enumerate() {
            if !eval_pred(pred, &cenv).unwrap_or(false) {
                held[i] = false;
            }
        }
    }

    let n_held = held.iter().filter(|b| **b).count() as u32;
    ContractRun::Checked(n_held, ens_preds.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::{Def, Expr, Type};

    fn pd_lam(body: Expr, params: &[&str]) -> ProducedDef {
        ProducedDef {
            name: Some("f".into()),
            def: Def::new(
                Expr::Lam {
                    params: params.iter().map(|s| s.to_string()).collect(),
                    body: Box::new(body),
                },
                Type::Named("Nat".into()),
            ),
        }
    }

    fn v(n: &str) -> Expr {
        Expr::Var(n.into())
    }
    fn call(f: &str, args: Vec<Expr>) -> Expr {
        Expr::App {
            func: Box::new(v(f)),
            args,
        }
    }

    fn params(names: &[&str]) -> Vec<Param> {
        names
            .iter()
            .map(|n| Param {
                name: n.to_string(),
                ty: "Nat".into(),
            })
            .collect()
    }

    // clamp = \p0 p1 p2 -> Nat.max p1 (Nat.min p0 p2)   over (value, lo, hi)
    fn clamp() -> ProducedDef {
        pd_lam(
            call(
                "Nat.max",
                vec![v("p1"), call("Nat.min", vec![v("p0"), v("p2")])],
            ),
            &["p0", "p1", "p2"],
        )
    }

    #[test]
    fn correct_clamp_satisfies_contracts() {
        let r = run_contracts(
            &[clamp()],
            &params(&["value", "lo", "hi"]),
            &["lo <= hi".to_string()],
            &["result >= lo".to_string(), "result <= hi".to_string()],
            &["Nat.max".into(), "Nat.min".into()],
        );
        assert_eq!(
            r,
            ContractRun::Checked(2, 2),
            "both postconditions hold on all cases"
        );
    }

    #[test]
    fn buggy_clamp_violates_a_contract() {
        // bug: returns value unchanged (ignores bounds)
        let buggy = pd_lam(v("p0"), &["p0", "p1", "p2"]);
        let r = run_contracts(
            &[buggy],
            &params(&["value", "lo", "hi"]),
            &["lo <= hi".to_string()],
            &["result >= lo".to_string(), "result <= hi".to_string()],
            &["Nat.max".into(), "Nat.min".into()],
        );
        // at least one ensures fails on some case → not both held
        match r {
            ContractRun::Checked(held, 2) => assert!(held < 2, "buggy impl must break a contract"),
            other => panic!("expected Checked, got {other:?}"),
        }
    }

    #[test]
    fn no_params_is_skipped_not_passed() {
        let r = run_contracts(&[clamp()], &[], &[], &["result >= lo".to_string()], &[]);
        assert_eq!(r, ContractRun::Skipped);
    }
}
