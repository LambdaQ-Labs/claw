//! Evaluate predicates against a value environment.
//!
//! An environment binds names (including primed post-state names and
//! dotted field paths like `from.balance`) to values. Built-in pure calls
//! (`List.len`, `Nat.max`, …) are interpreted here so contracts over them
//! are runnable without the full compiler.

use crate::{Arith, Op, PExpr, Pred};
use std::collections::BTreeMap;

/// Values a contract can reason about.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    List(Vec<Value>),
    /// A Result: Ok(inner) or Err(inner).
    Ok(Box<Value>),
    Err(Box<Value>),
}

/// Binding environment: name/path → value.
pub type Env = BTreeMap<String, Value>;

#[derive(Debug, PartialEq)]
pub enum EvalError {
    Unbound(String),
    TypeError(String),
    UnknownCall(String),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Unbound(n) => write!(f, "unbound `{n}`"),
            EvalError::TypeError(m) => write!(f, "type error: {m}"),
            EvalError::UnknownCall(n) => write!(f, "unknown call `{n}`"),
        }
    }
}

fn as_int(v: &Value) -> Result<i64, EvalError> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(EvalError::TypeError(format!("expected Int, got {other:?}"))),
    }
}

fn eval_expr(e: &PExpr, env: &Env) -> Result<Value, EvalError> {
    match e {
        PExpr::Int(n) => Ok(Value::Int(*n)),
        PExpr::Var(name) => env
            .get(name)
            .cloned()
            .ok_or_else(|| EvalError::Unbound(name.clone())),
        PExpr::Bin(op, a, b) => {
            let x = as_int(&eval_expr(a, env)?)?;
            let y = as_int(&eval_expr(b, env)?)?;
            Ok(Value::Int(match op {
                Arith::Add => x + y,
                Arith::Sub => x - y,
                Arith::Mul => x * y,
            }))
        }
        PExpr::Call(name, args) => {
            let vals: Result<Vec<Value>, _> = args.iter().map(|a| eval_expr(a, env)).collect();
            builtin_call(name, &vals?)
        }
    }
}

/// A small library of pure builtins contracts commonly reference.
fn builtin_call(name: &str, args: &[Value]) -> Result<Value, EvalError> {
    match (name, args) {
        ("List.len", [Value::List(xs)]) => Ok(Value::Int(xs.len() as i64)),
        ("Nat.max", [a, b]) => Ok(Value::Int(as_int(a)?.max(as_int(b)?))),
        ("Nat.min", [a, b]) => Ok(Value::Int(as_int(a)?.min(as_int(b)?))),
        ("Nat.add", [a, b]) => Ok(Value::Int(as_int(a)? + as_int(b)?)),
        _ => Err(EvalError::UnknownCall(format!("{name}/{}", args.len()))),
    }
}

/// Evaluate a predicate to a boolean under `env`.
pub fn eval_pred(p: &Pred, env: &Env) -> Result<bool, EvalError> {
    match p {
        Pred::Bool(b) => Ok(*b),
        Pred::Not(x) => Ok(!eval_pred(x, env)?),
        Pred::And(a, b) => Ok(eval_pred(a, env)? && eval_pred(b, env)?),
        Pred::Or(a, b) => Ok(eval_pred(a, env)? || eval_pred(b, env)?),
        // Vacuously true when the antecedent is false — standard implication.
        Pred::Implies(a, b) => {
            if eval_pred(a, env)? {
                eval_pred(b, env)
            } else {
                Ok(true)
            }
        }
        Pred::IsOk(name) => match env.get(name) {
            Some(Value::Ok(_)) => Ok(true),
            Some(_) => Ok(false),
            None => Err(EvalError::Unbound(name.clone())),
        },
        Pred::IsErr(name) => match env.get(name) {
            Some(Value::Err(_)) => Ok(true),
            Some(_) => Ok(false),
            None => Err(EvalError::Unbound(name.clone())),
        },
        Pred::Cmp(op, a, b) => {
            let x = as_int(&eval_expr(a, env)?)?;
            let y = as_int(&eval_expr(b, env)?)?;
            Ok(match op {
                Op::Eq => x == y,
                Op::Ne => x != y,
                Op::Le => x <= y,
                Op::Lt => x < y,
                Op::Ge => x >= y,
                Op::Gt => x > y,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_pred;

    fn env(pairs: &[(&str, Value)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn clamp_postcondition_holds_and_fails() {
        let ge = parse_pred("result >= lo").unwrap();
        let le = parse_pred("result <= hi").unwrap();
        let good = env(&[
            ("result", Value::Int(5)),
            ("lo", Value::Int(1)),
            ("hi", Value::Int(10)),
        ]);
        assert!(eval_pred(&ge, &good).unwrap() && eval_pred(&le, &good).unwrap());

        let bad = env(&[
            ("result", Value::Int(99)),
            ("lo", Value::Int(1)),
            ("hi", Value::Int(10)),
        ]);
        assert!(
            !eval_pred(&le, &bad).unwrap(),
            "should catch out-of-range output"
        );
    }

    #[test]
    fn transfer_postcondition_over_prime_state() {
        // ok(result) => from'.balance == from.balance - amt
        let p = parse_pred("ok(result) => from'.balance == from.balance - amt").unwrap();
        let e = env(&[
            ("result", Value::Ok(Box::new(Value::Int(0)))),
            ("from.balance", Value::Int(100)),
            ("from'.balance", Value::Int(70)),
            ("amt", Value::Int(30)),
        ]);
        assert!(eval_pred(&p, &e).unwrap());

        // wrong post-state balance → violation
        let bad = env(&[
            ("result", Value::Ok(Box::new(Value::Int(0)))),
            ("from.balance", Value::Int(100)),
            ("from'.balance", Value::Int(80)),
            ("amt", Value::Int(30)),
        ]);
        assert!(!eval_pred(&p, &bad).unwrap());
    }

    #[test]
    fn implication_is_vacuously_true_when_result_is_err() {
        let p = parse_pred("ok(result) => from'.balance == 0").unwrap();
        let e = env(&[
            ("result", Value::Err(Box::new(Value::Int(0)))),
            ("from'.balance", Value::Int(999)),
        ]);
        assert!(
            eval_pred(&p, &e).unwrap(),
            "err(result) makes ok-guarded ensures vacuous"
        );
    }

    #[test]
    fn builtin_list_len_in_contract() {
        let p = parse_pred("List.len(result) == List.len(input) + 1").unwrap();
        let e = env(&[
            (
                "result",
                Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            ),
            ("input", Value::List(vec![Value::Int(1), Value::Int(2)])),
        ]);
        assert!(eval_pred(&p, &e).unwrap());
    }
}
