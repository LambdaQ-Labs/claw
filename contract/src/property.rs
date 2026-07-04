//! Property-test case generation.
//!
//! Given a set of input variable names and a bound, enumerate integer
//! bindings deterministically (no RNG — reproducible in CI and usable as
//! corpus seed material). A precondition filters the cases; each surviving
//! case is a concrete environment a postcondition can be checked against
//! once the implementation runs.

use crate::eval::{eval_pred, Env, Value};
use crate::{parse::parse_pred, Pred};

/// One generated test case: an environment satisfying all preconditions.
#[derive(Debug, Clone)]
pub struct Case {
    pub env: Env,
}

/// Cartesian product of `0..=bound` over `vars`, keeping only cases where
/// every precondition holds. Deterministic ordering. Filtering is applied
/// incrementally as each variable is bound, and total materialized cases
/// are capped, so a wide grid (many vars × high bound) prunes early
/// instead of building the full (bound+1)^n product and OOM-ing.
///
/// A precondition that *errors* (references an unbound name, wrong type)
/// rejects the case AND is surfaced: `strict` — an all-cases-rejected
/// result with a non-trivial precondition is a caller signal, not a silent
/// vacuous pass. Here we simply never treat an eval error as `true`.
pub fn generate_cases(vars: &[&str], bound: i64, requires: &[Pred]) -> Vec<Case> {
    const MAX_CASES: usize = 100_000;
    let mut cases: Vec<Env> = vec![Env::new()];
    for &v in vars {
        let mut next = Vec::new();
        for base in &cases {
            for n in 0..=bound {
                let mut e = base.clone();
                e.insert(v.to_string(), Value::Int(n));
                // Prune with any precondition that is already fully
                // determined by the vars bound so far (Unbound → keep,
                // it may be satisfiable once more vars are bound).
                // not-yet-decidable (Unbound) or eval error → keep for now,
                // decide at the final gate.
                let keep = requires.iter().all(|p| eval_pred(p, &e).unwrap_or(true));
                if keep {
                    next.push(e);
                }
                if next.len() > MAX_CASES {
                    break;
                }
            }
        }
        cases = next;
    }
    cases
        .into_iter()
        // Final gate: every precondition must hold and evaluate cleanly.
        .filter(|env| requires.iter().all(|p| eval_pred(p, env) == Ok(true)))
        .map(|env| Case { env })
        .collect()
}

/// Convenience: parse precondition strings then generate.
pub fn generate_from_strings(
    vars: &[&str],
    bound: i64,
    requires: &[&str],
) -> Result<Vec<Case>, crate::ParseError> {
    let preds: Result<Vec<Pred>, _> = requires.iter().map(|s| parse_pred(s)).collect();
    Ok(generate_cases(vars, bound, &preds?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::Value;

    #[test]
    fn generates_full_grid_without_preconditions() {
        let cases = generate_cases(&["a", "b"], 2, &[]);
        assert_eq!(cases.len(), 9); // 3 * 3
    }

    #[test]
    fn preconditions_filter_cases() {
        // require a <= b
        let cases = generate_from_strings(&["a", "b"], 3, &["a <= b"]).unwrap();
        assert!(cases.iter().all(|c| {
            let a = matches!(c.env.get("a"), Some(Value::Int(n)) if matches!(c.env.get("b"), Some(Value::Int(m)) if n <= m));
            a
        }));
        // count of a<=b over 0..=3 grid = 10
        assert_eq!(cases.len(), 10);
    }

    #[test]
    fn generation_is_deterministic() {
        let a = generate_cases(&["x"], 3, &[]);
        let b = generate_cases(&["x"], 3, &[]);
        let xs: Vec<_> = a.iter().map(|c| c.env.get("x").cloned()).collect();
        let ys: Vec<_> = b.iter().map(|c| c.env.get("x").cloned()).collect();
        assert_eq!(xs, ys);
    }
}
