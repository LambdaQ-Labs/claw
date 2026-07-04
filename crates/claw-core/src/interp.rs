//! A small interpreter for the Claw expression core.
//!
//! Enough to *run* the definitions the benchmark produces — so a contract
//! stops being a comment and becomes a boolean you can check on real
//! inputs. References resolve through a `Resolver` (the CDB in practice);
//! a library of pure builtins covers the symbols the benchmark tasks use.
//!
//! This is the prototype interpreter; the production path is the compiler's
//! own evaluator. The value model is intentionally shared in shape with
//! claw-contract so contract predicates and program results speak the same
//! language.

use crate::{Expr, Hash, Lit};
use std::collections::BTreeMap;

/// Runtime values.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    List(Vec<Value>),
    /// A closure: params + body + captured environment.
    Closure(Vec<String>, Box<Expr>, Env),
    /// A builtin function referenced by name (applied by `apply`).
    Builtin(String),
    Ok(Box<Value>),
    Err(Box<Value>),
}

pub type Env = BTreeMap<String, Value>;

/// Resolves a definition hash to its expression (the CDB implements this).
pub trait Resolver {
    fn resolve(&self, hash: &Hash) -> Option<Expr>;
    /// The human name a hash is bound to, if any — used to dispatch builtins
    /// whose bodies we don't have (stdlib stubs in the benchmark scope).
    fn name_of(&self, hash: &Hash) -> Option<String>;
}

#[derive(Debug, PartialEq)]
pub enum RunError {
    Unbound(String),
    NotAFunction,
    Builtin(String),
    Unresolved(Hash),
    Depth,
}

const MAX_STEPS: u32 = 100_000;

pub fn eval(expr: &Expr, env: &Env, res: &dyn Resolver) -> Result<Value, RunError> {
    let mut steps = 0;
    eval_inner(expr, env, res, &mut steps)
}

fn eval_inner(
    expr: &Expr,
    env: &Env,
    res: &dyn Resolver,
    steps: &mut u32,
) -> Result<Value, RunError> {
    *steps += 1;
    if *steps > MAX_STEPS {
        return Err(RunError::Depth);
    }
    match expr {
        Expr::Lit(Lit::Int(n)) => Ok(Value::Int(*n)),
        Expr::Lit(Lit::Str(s)) => Ok(Value::Str(s.clone())),
        Expr::Var(v) => env
            .get(v)
            .cloned()
            .ok_or_else(|| RunError::Unbound(v.clone())),
        Expr::Ref(h) => {
            // A stdlib stub in scope resolves to its builtin by name;
            // otherwise inline the referenced definition's expression.
            if let Some(name) = res.name_of(h) {
                if is_builtin(&name) {
                    return Ok(Value::Builtin(name));
                }
            }
            match res.resolve(h) {
                Some(e) => eval_inner(&e, env, res, steps),
                None => Err(RunError::Unresolved(h.clone())),
            }
        }
        Expr::Lam { params, body } => Ok(Value::Closure(params.clone(), body.clone(), env.clone())),
        Expr::App { func, args } => {
            let f = eval_inner(func, env, res, steps)?;
            let mut argv = Vec::with_capacity(args.len());
            for a in args {
                argv.push(eval_inner(a, env, res, steps)?);
            }
            apply(f, argv, res, steps)
        }
    }
}

fn apply(
    f: Value,
    args: Vec<Value>,
    res: &dyn Resolver,
    steps: &mut u32,
) -> Result<Value, RunError> {
    match f {
        Value::Closure(params, body, captured) => {
            let mut env = captured;
            for (p, a) in params.iter().zip(args) {
                env.insert(p.clone(), a);
            }
            eval_inner(&body, &env, res, steps)
        }
        Value::Builtin(name) => builtin(&name, &args),
        _ => Err(RunError::NotAFunction),
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "Nat.add"
            | "Nat.mul"
            | "Nat.sub"
            | "Nat.max"
            | "Nat.min"
            | "Nat.isZero"
            | "Nat.eq"
            | "Str.concat"
            | "List.len"
            | "Bool.if"
    )
}

fn as_int(v: &Value) -> Result<i64, RunError> {
    match v {
        Value::Int(n) => Ok(*n),
        _ => Err(RunError::Builtin("expected Int".into())),
    }
}

fn builtin(name: &str, args: &[Value]) -> Result<Value, RunError> {
    match (name, args) {
        ("Nat.add", [a, b]) => Ok(Value::Int(as_int(a)? + as_int(b)?)),
        ("Nat.mul", [a, b]) => Ok(Value::Int(as_int(a)? * as_int(b)?)),
        ("Nat.sub", [a, b]) => Ok(Value::Int((as_int(a)? - as_int(b)?).max(0))),
        ("Nat.max", [a, b]) => Ok(Value::Int(as_int(a)?.max(as_int(b)?))),
        ("Nat.min", [a, b]) => Ok(Value::Int(as_int(a)?.min(as_int(b)?))),
        ("Nat.isZero", [a]) => Ok(Value::Bool(as_int(a)? == 0)),
        ("Nat.eq", [a, b]) => Ok(Value::Bool(as_int(a)? == as_int(b)?)),
        ("Str.concat", [Value::Str(a), Value::Str(b)]) => Ok(Value::Str(format!("{a}{b}"))),
        ("List.len", [Value::List(xs)]) => Ok(Value::Int(xs.len() as i64)),
        ("Bool.if", [Value::Bool(c), t, e]) => Ok(if *c { t.clone() } else { e.clone() }),
        _ => Err(RunError::Builtin(format!("{name}/{}", args.len()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Type;

    /// A resolver where every hash maps to a builtin of the same name.
    struct BuiltinResolver;
    impl Resolver for BuiltinResolver {
        fn resolve(&self, _: &Hash) -> Option<Expr> {
            None
        }
        fn name_of(&self, h: &Hash) -> Option<String> {
            Some(h.0.clone()) // hash string IS the name in these tests
        }
    }

    fn refb(name: &str) -> Expr {
        Expr::Ref(Hash(name.into()))
    }

    #[test]
    fn evaluates_literal() {
        assert_eq!(
            eval(&Expr::Lit(Lit::Int(42)), &Env::new(), &BuiltinResolver),
            Ok(Value::Int(42))
        );
    }

    #[test]
    fn applies_builtin_through_ref() {
        // Nat.add 2 3
        let e = Expr::App {
            func: Box::new(refb("Nat.add")),
            args: vec![Expr::Lit(Lit::Int(2)), Expr::Lit(Lit::Int(3))],
        };
        assert_eq!(eval(&e, &Env::new(), &BuiltinResolver), Ok(Value::Int(5)));
    }

    #[test]
    fn runs_a_lambda_double() {
        // (\p0 -> Nat.add p0 p0) 21  == 42
        let double = Expr::Lam {
            params: vec!["p0".into()],
            body: Box::new(Expr::App {
                func: Box::new(refb("Nat.add")),
                args: vec![Expr::Var("p0".into()), Expr::Var("p0".into())],
            }),
        };
        let call = Expr::App {
            func: Box::new(double),
            args: vec![Expr::Lit(Lit::Int(21))],
        };
        assert_eq!(
            eval(&call, &Env::new(), &BuiltinResolver),
            Ok(Value::Int(42))
        );
    }

    #[test]
    fn clamp_via_min_max() {
        // clamp = \p0 p1 p2 -> Nat.max p1 (Nat.min p0 p2)   (x lo hi)
        let clamp = Expr::Lam {
            params: vec!["p0".into(), "p1".into(), "p2".into()],
            body: Box::new(Expr::App {
                func: Box::new(refb("Nat.max")),
                args: vec![
                    Expr::Var("p1".into()),
                    Expr::App {
                        func: Box::new(refb("Nat.min")),
                        args: vec![Expr::Var("p0".into()), Expr::Var("p2".into())],
                    },
                ],
            }),
        };
        let call = |x: i64, lo: i64, hi: i64| Expr::App {
            func: Box::new(clamp.clone()),
            args: vec![
                Expr::Lit(Lit::Int(x)),
                Expr::Lit(Lit::Int(lo)),
                Expr::Lit(Lit::Int(hi)),
            ],
        };
        assert_eq!(
            eval(&call(99, 1, 10), &Env::new(), &BuiltinResolver),
            Ok(Value::Int(10))
        );
        assert_eq!(
            eval(&call(5, 1, 10), &Env::new(), &BuiltinResolver),
            Ok(Value::Int(5))
        );
        assert_eq!(
            eval(&call(0, 1, 10), &Env::new(), &BuiltinResolver),
            Ok(Value::Int(1))
        );
    }

    #[test]
    fn unbound_variable_errors() {
        let _ = Type::Named("x".into()); // keep import used
        assert_eq!(
            eval(&Expr::Var("nope".into()), &Env::new(), &BuiltinResolver),
            Err(RunError::Unbound("nope".into()))
        );
    }
}
