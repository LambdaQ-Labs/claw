//! claw-core — the shared vocabulary of the Claw toolchain.
//!
//! Defines the minimal AST, type representation, and content-addressed
//! hashing that the code-as-database (cdb), constraint server, and bench
//! grader all build on. The full compiler (vendored Roc, `compiler/`)
//! will eventually lower into these structures; until then this is the
//! substrate the P2 thesis is prototyped and measured against.

pub mod interp;
pub mod parse;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Content address of a definition: blake3 of its canonical serialization.
/// Identity of code. Names are mutable pointers to hashes (see cdb).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hash(pub String);

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", &self.0[..self.0.len().min(8)])
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Minimal structural type language.
///
/// Rich enough to express the signatures the constraint server needs to
/// unify against (`candidates(type, scope)`); deliberately smaller than
/// the eventual full Claw type system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    /// A concrete named type: `Nat`, `Str`, `Account`.
    Named(String),
    /// A type variable: unifies with anything. `a`, `b`.
    Var(String),
    /// Type application: `Result Ledger TransferErr`, `List Nat`.
    App(String, Vec<Type>),
    /// Function type: `Nat, Nat -> Result Nat MathErr`.
    Fn(Vec<Type>, Box<Type>),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Named(n) => write!(f, "{n}"),
            Type::Var(v) => write!(f, "{v}"),
            Type::App(head, args) => {
                write!(f, "{head}")?;
                for a in args {
                    match a {
                        Type::Fn(..) | Type::App(..) => write!(f, " ({a})")?,
                        _ => write!(f, " {a}")?,
                    }
                }
                Ok(())
            }
            Type::Fn(params, ret) => {
                // Fn-typed params need parens or the arrow re-parses wrong:
                // `(a -> a), a -> a`, not `a -> a, a -> a`.
                let ps: Vec<String> = params
                    .iter()
                    .map(|p| match p {
                        Type::Fn(..) => format!("({p})"),
                        _ => p.to_string(),
                    })
                    .collect();
                write!(f, "{} -> {}", ps.join(", "), ret)
            }
        }
    }
}

/// Literals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lit {
    Int(i64),
    Str(String),
}

/// Minimal expression language.
///
/// The key constructor is `Ref(Hash)`: definitions reference each other by
/// content hash, never by name. That is what makes rename O(1) and makes
/// "referenced symbol doesn't exist" mechanically detectable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// Local variable (lambda parameter).
    Var(String),
    /// Reference to another definition by content hash.
    Ref(Hash),
    /// Literal.
    Lit(Lit),
    /// Lambda: `\a, b -> body`.
    Lam {
        params: Vec<String>,
        body: Box<Expr>,
    },
    /// Application: `f x y`.
    App { func: Box<Expr>, args: Vec<Expr> },
}

impl Expr {
    /// All definition hashes this expression references (its dependencies).
    pub fn refs(&self) -> Vec<Hash> {
        let mut out = Vec::new();
        self.walk_refs(&mut out);
        out.sort();
        out.dedup();
        out
    }

    fn walk_refs(&self, out: &mut Vec<Hash>) {
        match self {
            Expr::Ref(h) => out.push(h.clone()),
            Expr::Lam { body, .. } => body.walk_refs(out),
            Expr::App { func, args } => {
                func.walk_refs(out);
                for a in args {
                    a.walk_refs(out);
                }
            }
            Expr::Var(_) | Expr::Lit(_) => {}
        }
    }

    /// Free (unbound) local variables — anything here that isn't a lambda
    /// param is an unresolved name, i.e. a candidate hallucination.
    pub fn free_vars(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.walk_free(&mut Vec::new(), &mut out);
        out.sort();
        out.dedup();
        out
    }

    fn walk_free(&self, bound: &mut Vec<String>, out: &mut Vec<String>) {
        match self {
            Expr::Var(v) => {
                if !bound.contains(v) {
                    out.push(v.clone());
                }
            }
            Expr::Lam { params, body } => {
                let n = params.len();
                bound.extend(params.iter().cloned());
                body.walk_free(bound, out);
                bound.truncate(bound.len() - n);
            }
            Expr::App { func, args } => {
                func.walk_free(bound, out);
                for a in args {
                    a.walk_free(bound, out);
                }
            }
            Expr::Ref(_) | Expr::Lit(_) => {}
        }
    }
}

/// A definition: the unit of code. Content-addressed; nameless.
/// (Names live in the cdb `names` table as mutable pointers.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Def {
    pub expr: Expr,
    pub ty: Type,
    /// Effect row, e.g. ["Net", "Read"]. Empty = pure.
    pub effects: Vec<String>,
    pub deprecated: bool,
    pub doc: String,
}

impl Def {
    pub fn new(expr: Expr, ty: Type) -> Self {
        Def {
            expr,
            ty,
            effects: Vec::new(),
            deprecated: false,
            doc: String::new(),
        }
    }

    /// Content address: blake3 over the canonical serialization of
    /// (expr, ty, effects). Docs/deprecation are metadata — not identity.
    pub fn hash(&self) -> Hash {
        let canonical = serde_json::to_vec(&(&self.expr, &self.ty, &self.effects))
            .expect("Def serialization is infallible");
        Hash(blake3::hash(&canonical).to_hex().to_string())
    }

    /// Dependencies = every definition this one references.
    pub fn deps(&self) -> Vec<Hash> {
        self.expr.refs()
    }
}

/// A substitution from type variables to types.
pub type Subst = BTreeMap<String, Type>;

/// Structural unification (MVP: no occurs check — fine for the finite,
/// non-recursive signatures the prototype handles).
///
/// Returns the substitution under which `a` and `b` are equal, or None.
pub fn unify(a: &Type, b: &Type) -> Option<Subst> {
    let mut subst = Subst::new();
    if unify_into(a, b, &mut subst) {
        Some(subst)
    } else {
        None
    }
}

fn resolve(t: &Type, subst: &Subst) -> Type {
    match t {
        Type::Var(v) => match subst.get(v) {
            Some(bound) => resolve(bound, subst),
            None => t.clone(),
        },
        _ => t.clone(),
    }
}

fn unify_into(a: &Type, b: &Type, subst: &mut Subst) -> bool {
    let a = resolve(a, subst);
    let b = resolve(b, subst);
    match (&a, &b) {
        (Type::Var(v), other) | (other, Type::Var(v)) => {
            if let Type::Var(v2) = other {
                if v == v2 {
                    return true;
                }
            }
            subst.insert(v.clone(), other.clone());
            true
        }
        (Type::Named(x), Type::Named(y)) => x == y,
        (Type::App(h1, args1), Type::App(h2, args2)) => {
            h1 == h2
                && args1.len() == args2.len()
                && args1
                    .iter()
                    .zip(args2)
                    .all(|(x, y)| unify_into(x, y, subst))
        }
        (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
            p1.len() == p2.len()
                && p1.iter().zip(p2).all(|(x, y)| unify_into(x, y, subst))
                && unify_into(r1, r2, subst)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    #[test]
    fn hash_is_stable_and_content_addressed() {
        let d1 = Def::new(Expr::Lit(Lit::Int(42)), named("Nat"));
        let d2 = Def::new(Expr::Lit(Lit::Int(42)), named("Nat"));
        let d3 = Def::new(Expr::Lit(Lit::Int(43)), named("Nat"));
        assert_eq!(d1.hash(), d2.hash(), "same content, same hash");
        assert_ne!(d1.hash(), d3.hash(), "different content, different hash");
    }

    #[test]
    fn metadata_does_not_change_identity() {
        let mut d1 = Def::new(Expr::Lit(Lit::Int(1)), named("Nat"));
        let d_hash = d1.hash();
        d1.doc = "documented".into();
        d1.deprecated = true;
        assert_eq!(
            d1.hash(),
            d_hash,
            "doc/deprecation are metadata, not identity"
        );
    }

    #[test]
    fn deps_are_collected_from_refs() {
        let dep = Def::new(Expr::Lit(Lit::Int(1)), named("Nat"));
        let h = dep.hash();
        let user = Def::new(
            Expr::App {
                func: Box::new(Expr::Ref(h.clone())),
                args: vec![Expr::Lit(Lit::Int(2))],
            },
            named("Nat"),
        );
        assert_eq!(user.deps(), vec![h]);
    }

    #[test]
    fn free_vars_sees_through_lambdas() {
        // \x -> mystery x   — `mystery` is free (unresolved), `x` is bound.
        let e = Expr::Lam {
            params: vec!["x".into()],
            body: Box::new(Expr::App {
                func: Box::new(Expr::Var("mystery".into())),
                args: vec![Expr::Var("x".into())],
            }),
        };
        assert_eq!(e.free_vars(), vec!["mystery".to_string()]);
    }

    #[test]
    fn unify_var_binds_to_anything() {
        let q = Type::Fn(
            vec![named("Nat"), named("Nat")],
            Box::new(Type::Var("a".into())),
        );
        let t = Type::Fn(
            vec![named("Nat"), named("Nat")],
            Box::new(Type::App(
                "Result".into(),
                vec![named("Nat"), named("MathErr")],
            )),
        );
        let s = unify(&q, &t).expect("should unify");
        assert_eq!(
            s.get("a"),
            Some(&Type::App(
                "Result".into(),
                vec![named("Nat"), named("MathErr")]
            ))
        );
    }

    #[test]
    fn unify_rejects_mismatched_shapes() {
        assert!(unify(&named("Nat"), &named("Str")).is_none());
        let f1 = Type::Fn(vec![named("Nat")], Box::new(named("Nat")));
        let f2 = Type::Fn(vec![named("Nat"), named("Nat")], Box::new(named("Nat")));
        assert!(unify(&f1, &f2).is_none(), "arity mismatch");
    }

    #[test]
    fn type_display_reads_naturally() {
        let t = Type::Fn(
            vec![named("Account"), named("Nat")],
            Box::new(Type::App(
                "Result".into(),
                vec![named("Ledger"), named("Err")],
            )),
        );
        assert_eq!(t.to_string(), "Account, Nat -> Result Ledger Err");
    }
}
