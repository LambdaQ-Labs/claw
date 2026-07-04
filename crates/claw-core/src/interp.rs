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
    /// A record: field name → value.
    Record(BTreeMap<String, Value>),
    /// A tag / variant: name + payload (e.g. a pipeline stage `Won`).
    Tag(String, Vec<Value>),
}

pub type Env = BTreeMap<String, Value>;

/// Resolves a definition hash to its expression (the CDB implements this).
pub trait Resolver {
    fn resolve(&self, hash: &Hash) -> Option<Expr>;
    /// The human name a hash is bound to, if any — used to dispatch builtins
    /// whose bodies we don't have (stdlib stubs in the benchmark scope).
    fn name_of(&self, hash: &Hash) -> Option<String>;
    /// Resolve a free name to a definition's body (a top-level def
    /// referenced by name). Lets the interpreter run real lowered bodies
    /// from the CDB, where cross-def references are `Var(name)`, not `Ref`.
    fn resolve_name(&self, _name: &str) -> Option<Expr> {
        None
    }
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
// Native recursion-depth cap. eval_inner/apply recurse on the Rust stack;
// without a depth bound a term like the omega combinator overflows the
// native stack (SIGSEGV) before MAX_STEPS trips. A modest cap returns
// RunError::Depth cleanly, well under the default 8 MB stack.
const MAX_DEPTH: u32 = 1024;

struct Budget {
    steps: u32,
    depth: u32,
}

pub fn eval(expr: &Expr, env: &Env, res: &dyn Resolver) -> Result<Value, RunError> {
    let mut b = Budget { steps: 0, depth: 0 };
    eval_inner(expr, env, res, &mut b)
}

fn eval_inner(
    expr: &Expr,
    env: &Env,
    res: &dyn Resolver,
    b: &mut Budget,
) -> Result<Value, RunError> {
    b.steps += 1;
    if b.steps > MAX_STEPS || b.depth > MAX_DEPTH {
        return Err(RunError::Depth);
    }
    b.depth += 1;
    let out = eval_step(expr, env, res, b);
    b.depth -= 1;
    out
}

fn eval_step(
    expr: &Expr,
    env: &Env,
    res: &dyn Resolver,
    b: &mut Budget,
) -> Result<Value, RunError> {
    match expr {
        Expr::Lit(Lit::Int(n)) => Ok(Value::Int(*n)),
        Expr::Lit(Lit::Str(s)) => Ok(Value::Str(s.clone())),
        Expr::Var(v) => {
            if let Some(val) = env.get(v) {
                Ok(val.clone())
            } else if is_builtin(v) {
                Ok(Value::Builtin(v.clone()))
            } else if let Some(body) = res.resolve_name(v) {
                // A top-level def referenced by name: a closed term, so it
                // evaluates in a fresh environment (captures nothing here).
                eval_inner(&body, &Env::new(), res, b)
            } else {
                Err(RunError::Unbound(v.clone()))
            }
        }
        Expr::Ref(h) => {
            // A stdlib stub in scope resolves to its builtin by name;
            // otherwise inline the referenced definition's expression.
            if let Some(name) = res.name_of(h) {
                if is_builtin(&name) {
                    return Ok(Value::Builtin(name));
                }
            }
            match res.resolve(h) {
                Some(e) => eval_inner(&e, env, res, b),
                None => Err(RunError::Unresolved(h.clone())),
            }
        }
        Expr::Lam { params, body } => Ok(Value::Closure(params.clone(), body.clone(), env.clone())),
        Expr::App { func, args } => {
            let f = eval_inner(func, env, res, b)?;
            let mut argv = Vec::with_capacity(args.len());
            for a in args {
                argv.push(eval_inner(a, env, res, b)?);
            }
            apply(f, argv, res, b)
        }
        // Lazy: evaluate the condition, then only the taken branch. A
        // non-taken branch that would error (or diverge) does not run.
        Expr::If { cond, then, els } => match eval_inner(cond, env, res, b)? {
            Value::Bool(true) => eval_inner(then, env, res, b),
            Value::Bool(false) => eval_inner(els, env, res, b),
            _ => Err(RunError::Builtin("if: condition is not a Bool".into())),
        },
        Expr::Let { name, value, body } => {
            let v = eval_inner(value, env, res, b)?;
            let mut env2 = env.clone();
            env2.insert(name.clone(), v);
            eval_inner(body, &env2, res, b)
        }
        Expr::Record(fields) => {
            let mut map = BTreeMap::new();
            for (name, e) in fields {
                map.insert(name.clone(), eval_inner(e, env, res, b)?);
            }
            Ok(Value::Record(map))
        }
        Expr::Field(e, name) => match eval_inner(e, env, res, b)? {
            Value::Record(map) => map
                .get(name)
                .cloned()
                .ok_or_else(|| RunError::Builtin(format!("no field `{name}`"))),
            _ => Err(RunError::Builtin(format!("field `{name}` on a non-record"))),
        },
        Expr::Tag(name, args) => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_inner(a, env, res, b)?);
            }
            // Ok/Err map to the dedicated values so contracts see them.
            Ok(match (name.as_str(), vals.len()) {
                ("Ok", 1) => Value::Ok(Box::new(vals.into_iter().next().unwrap())),
                ("Err", 1) => Value::Err(Box::new(vals.into_iter().next().unwrap())),
                _ => Value::Tag(name.clone(), vals),
            })
        }
        Expr::Match(scrut, arms) => {
            let v = eval_inner(scrut, env, res, b)?;
            for (pat, body) in arms {
                let mut binds = Vec::new();
                if match_pat(pat, &v, &mut binds) {
                    let mut env2 = env.clone();
                    for (k, val) in binds {
                        env2.insert(k, val);
                    }
                    return eval_inner(body, &env2, res, b);
                }
            }
            Err(RunError::Builtin("match: no arm matched".into()))
        }
    }
}

/// Try to match `pat` against `v`; on success, collect its bindings.
fn match_pat(pat: &crate::Pat, v: &Value, binds: &mut Vec<(String, Value)>) -> bool {
    use crate::{Lit, Pat};
    match pat {
        Pat::Wild => true,
        Pat::Var(name) => {
            binds.push((name.clone(), v.clone()));
            true
        }
        Pat::Lit(Lit::Int(n)) => matches!(v, Value::Int(m) if m == n),
        Pat::Lit(Lit::Str(s)) => matches!(v, Value::Str(t) if t == s),
        Pat::Tag(name, subs) => {
            // Ok/Err are dedicated values; other tags are Value::Tag.
            let (vname, vargs): (&str, Vec<Value>) = match v {
                Value::Ok(x) => ("Ok", vec![(**x).clone()]),
                Value::Err(x) => ("Err", vec![(**x).clone()]),
                Value::Tag(n, a) => (n.as_str(), a.clone()),
                _ => return false,
            };
            if vname != name || vargs.len() != subs.len() {
                return false;
            }
            subs.iter()
                .zip(&vargs)
                .all(|(sp, sv)| match_pat(sp, sv, binds))
        }
    }
}

fn apply(
    f: Value,
    args: Vec<Value>,
    res: &dyn Resolver,
    b: &mut Budget,
) -> Result<Value, RunError> {
    match f {
        Value::Closure(params, body, captured) => {
            // Arity mismatch is a diagnosable error, not a silent mis-bind.
            if params.len() != args.len() {
                return Err(RunError::Builtin(format!(
                    "arity: closure expects {} arg(s), got {}",
                    params.len(),
                    args.len()
                )));
            }
            let mut env = captured;
            for (p, a) in params.iter().zip(args) {
                env.insert(p.clone(), a);
            }
            eval_inner(&body, &env, res, b)
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
            // Operator/method names the compiler lowers to (real .claw bodies).
            | "plus"
            | "minus"
            | "times"
            | "is_lt"
            | "is_le"
            | "is_gt"
            | "is_ge"
            | "is_eq"
            | "to_str"
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
        // Saturating (consistent with Nat.sub's clamp): a Nat never wraps
        // negative, and overflow can't panic the interpreter.
        ("Nat.add", [a, b]) => Ok(Value::Int(as_int(a)?.saturating_add(as_int(b)?))),
        ("Nat.mul", [a, b]) => Ok(Value::Int(as_int(a)?.saturating_mul(as_int(b)?))),
        ("Nat.sub", [a, b]) => Ok(Value::Int((as_int(a)? - as_int(b)?).max(0))),
        ("Nat.max", [a, b]) => Ok(Value::Int(as_int(a)?.max(as_int(b)?))),
        ("Nat.min", [a, b]) => Ok(Value::Int(as_int(a)?.min(as_int(b)?))),
        ("Nat.isZero", [a]) => Ok(Value::Bool(as_int(a)? == 0)),
        ("Nat.eq", [a, b]) => Ok(Value::Bool(as_int(a)? == as_int(b)?)),
        ("Str.concat", [Value::Str(a), Value::Str(b)]) => Ok(Value::Str(format!("{a}{b}"))),
        ("List.len", [Value::List(xs)]) => Ok(Value::Int(xs.len() as i64)),
        ("Bool.if", [Value::Bool(c), t, e]) => Ok(if *c { t.clone() } else { e.clone() }),
        // Compiler operator/method lowerings, over Int.
        ("plus", [a, b]) => Ok(Value::Int(as_int(a)?.saturating_add(as_int(b)?))),
        ("minus", [a, b]) => Ok(Value::Int(as_int(a)?.saturating_sub(as_int(b)?))),
        ("times", [a, b]) => Ok(Value::Int(as_int(a)?.saturating_mul(as_int(b)?))),
        ("is_lt", [a, b]) => Ok(Value::Bool(as_int(a)? < as_int(b)?)),
        ("is_le", [a, b]) => Ok(Value::Bool(as_int(a)? <= as_int(b)?)),
        ("is_gt", [a, b]) => Ok(Value::Bool(as_int(a)? > as_int(b)?)),
        ("is_ge", [a, b]) => Ok(Value::Bool(as_int(a)? >= as_int(b)?)),
        ("is_eq", [a, b]) => Ok(Value::Bool(as_int(a)? == as_int(b)?)),
        ("to_str", [Value::Int(n)]) => Ok(Value::Str(n.to_string())),
        ("to_str", [Value::Str(s)]) => Ok(Value::Str(s.clone())),
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

    #[test]
    fn record_field_access() {
        use crate::Expr;
        // ({ x: 10, y: 20 }).y == 20
        let rec = Expr::Record(vec![
            ("x".into(), Expr::Lit(Lit::Int(10))),
            ("y".into(), Expr::Lit(Lit::Int(20))),
        ]);
        let e = Expr::Field(Box::new(rec), "y".into());
        assert_eq!(eval(&e, &Env::new(), &BuiltinResolver), Ok(Value::Int(20)));
    }

    #[test]
    fn match_on_tag_union_binds_and_selects() {
        use crate::{Expr, Pat};
        // match Stage("open") { Won => 1, Stage(s) => 2, _ => 0 }  == 2, binds s
        let scrut = Expr::Tag("Stage".into(), vec![Expr::Lit(Lit::Str("open".into()))]);
        let e = Expr::Match(
            Box::new(scrut),
            vec![
                (Pat::Tag("Won".into(), vec![]), Expr::Lit(Lit::Int(1))),
                (
                    Pat::Tag("Stage".into(), vec![Pat::Var("s".into())]),
                    Expr::Lit(Lit::Int(2)),
                ),
                (Pat::Wild, Expr::Lit(Lit::Int(0))),
            ],
        );
        assert_eq!(eval(&e, &Env::new(), &BuiltinResolver), Ok(Value::Int(2)));
    }

    #[test]
    fn resolves_free_names_via_the_resolver_and_real_ops() {
        // A resolver that maps `inc` to its lowered body: |n| plus(n, 1).
        struct NameRes;
        impl Resolver for NameRes {
            fn resolve(&self, _: &Hash) -> Option<Expr> {
                None
            }
            fn name_of(&self, _: &Hash) -> Option<String> {
                None
            }
            fn resolve_name(&self, name: &str) -> Option<Expr> {
                if name == "inc" {
                    Some(Expr::Lam {
                        params: vec!["n".into()],
                        body: Box::new(Expr::App {
                            func: Box::new(Expr::Var("plus".into())),
                            args: vec![Expr::Var("n".into()), Expr::Lit(Lit::Int(1))],
                        }),
                    })
                } else {
                    None
                }
            }
        }
        // inc(inc(5)) == 7, resolving `inc` by name and `plus` as a builtin.
        let e = Expr::App {
            func: Box::new(Expr::Var("inc".into())),
            args: vec![Expr::App {
                func: Box::new(Expr::Var("inc".into())),
                args: vec![Expr::Lit(Lit::Int(5))],
            }],
        };
        assert_eq!(eval(&e, &Env::new(), &NameRes), Ok(Value::Int(7)));
    }

    #[test]
    fn if_is_lazy_untaken_branch_never_runs() {
        // if True then 1 else <unbound> — the else must NOT be evaluated.
        let e = Expr::If {
            cond: Box::new(Expr::Lit(Lit::Int(1))), // stand-in bool below
            then: Box::new(Expr::Lit(Lit::Int(1))),
            els: Box::new(Expr::Var("would_error".into())),
        };
        // condition must be a Bool; use a real bool via a builtin.
        let cond_true = Expr::App {
            func: Box::new(refb("Nat.isZero")),
            args: vec![Expr::Lit(Lit::Int(0))],
        };
        let taken = Expr::If {
            cond: Box::new(cond_true),
            then: Box::new(Expr::Lit(Lit::Int(42))),
            els: Box::new(Expr::Var("would_error".into())),
        };
        assert_eq!(
            eval(&taken, &Env::new(), &BuiltinResolver),
            Ok(Value::Int(42))
        );
        // a non-bool condition is a clean error, not a panic
        assert!(matches!(
            eval(&e, &Env::new(), &BuiltinResolver),
            Err(RunError::Builtin(_))
        ));
    }

    #[test]
    fn let_binds_in_body() {
        // let x = 20 in Nat.add x x  == 40
        let e = Expr::Let {
            name: "x".into(),
            value: Box::new(Expr::Lit(Lit::Int(20))),
            body: Box::new(Expr::App {
                func: Box::new(refb("Nat.add")),
                args: vec![Expr::Var("x".into()), Expr::Var("x".into())],
            }),
        };
        assert_eq!(eval(&e, &Env::new(), &BuiltinResolver), Ok(Value::Int(40)));
    }

    #[test]
    fn omega_returns_depth_not_stack_overflow() {
        // (\x -> x x)(\x -> x x) — diverges. Must return RunError::Depth,
        // not blow the native stack (SIGSEGV). Regression for the review's
        // #1 bug (step bound didn't cap native recursion).
        let self_app = Expr::Lam {
            params: vec!["x".into()],
            body: Box::new(Expr::App {
                func: Box::new(Expr::Var("x".into())),
                args: vec![Expr::Var("x".into())],
            }),
        };
        let omega = Expr::App {
            func: Box::new(self_app.clone()),
            args: vec![self_app],
        };
        assert_eq!(
            eval(&omega, &Env::new(), &BuiltinResolver),
            Err(RunError::Depth)
        );
    }

    #[test]
    fn arity_mismatch_is_an_error_not_a_misbind() {
        // (\p0 p1 -> p0) applied to one arg → arity error, not Unbound(p1).
        let f = Expr::Lam {
            params: vec!["p0".into(), "p1".into()],
            body: Box::new(Expr::Var("p0".into())),
        };
        let call = Expr::App {
            func: Box::new(f),
            args: vec![Expr::Lit(Lit::Int(1))],
        };
        assert!(matches!(
            eval(&call, &Env::new(), &BuiltinResolver),
            Err(RunError::Builtin(_))
        ));
    }
}
