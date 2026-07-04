//! claw-core — the shared vocabulary of the Claw toolchain.
//!
//! Defines the minimal AST, type representation, and content-addressed
//! hashing that the code-as-database (cdb), constraint server, and bench
//! grader all build on. The full compiler (vendored Roc, `compiler/`)
//! will eventually lower into these structures; until then this is the
//! substrate the P2 thesis is prototyped and measured against.

pub mod interp;
pub mod parse;
pub mod render;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Content address of a definition: blake3 of its canonical serialization.
/// Identity of code. Names are mutable pointers to hashes (see cdb).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hash(pub String);

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Truncate by chars, not bytes — a byte slice could split a
        // multibyte char and panic (real blake3 hashes are ASCII, but
        // Hash wraps an arbitrary String).
        let short: String = self.0.chars().take(8).collect();
        write!(f, "#{short}")
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
    /// Conditional: `if cond then a else b`. Lazy — only the taken branch
    /// is evaluated (unlike a `Bool.if` builtin over pre-evaluated args).
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    /// A let-binding in a block: `name = value; body`. `name` is bound in
    /// `body` (not in `value`).
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// A record literal: `{ field: expr, ... }`.
    Record(Vec<(String, Expr)>),
    /// Field access: `expr.field`.
    Field(Box<Expr>, String),
    /// A tag / variant constructor: `Tag` or `Tag(args)` (e.g. a pipeline
    /// stage `Won`, or `Ok(x)`).
    Tag(String, Vec<Expr>),
    /// Pattern match: `match scrutinee { pat => body, ... }`.
    Match(Box<Expr>, Vec<(Pat, Expr)>),
}

/// A match pattern (minimal: enough for tag-union state machines + guards).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pat {
    /// `_` — matches anything, binds nothing.
    Wild,
    /// A variable — matches anything, binds the value to this name.
    Var(String),
    /// A literal — matches by equality.
    Lit(Lit),
    /// A tag with sub-patterns: `Ok(x)`, `Stage(s)`, `Won`.
    Tag(String, Vec<Pat>),
}

impl Pat {
    /// Names this pattern binds (added to scope in the arm body).
    fn binds(&self, out: &mut Vec<String>) {
        match self {
            Pat::Var(v) => out.push(v.clone()),
            Pat::Tag(_, subs) => {
                for s in subs {
                    s.binds(out);
                }
            }
            Pat::Wild | Pat::Lit(_) => {}
        }
    }
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
            Expr::If { cond, then, els } => {
                cond.walk_refs(out);
                then.walk_refs(out);
                els.walk_refs(out);
            }
            Expr::Let { value, body, .. } => {
                value.walk_refs(out);
                body.walk_refs(out);
            }
            Expr::Record(fields) => {
                for (_, e) in fields {
                    e.walk_refs(out);
                }
            }
            Expr::Field(e, _) => e.walk_refs(out),
            Expr::Tag(_, args) => {
                for a in args {
                    a.walk_refs(out);
                }
            }
            Expr::Match(scrut, arms) => {
                scrut.walk_refs(out);
                for (_, body) in arms {
                    body.walk_refs(out);
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
            Expr::If { cond, then, els } => {
                cond.walk_free(bound, out);
                then.walk_free(bound, out);
                els.walk_free(bound, out);
            }
            Expr::Let { name, value, body } => {
                // `value` is in the outer scope; `name` is bound in `body`.
                value.walk_free(bound, out);
                bound.push(name.clone());
                body.walk_free(bound, out);
                bound.pop();
            }
            Expr::Record(fields) => {
                for (_, e) in fields {
                    e.walk_free(bound, out);
                }
            }
            Expr::Field(e, _) => e.walk_free(bound, out),
            Expr::Tag(_, args) => {
                for a in args {
                    a.walk_free(bound, out);
                }
            }
            Expr::Match(scrut, arms) => {
                scrut.walk_free(bound, out);
                for (pat, body) in arms {
                    // pattern bindings are in scope only in that arm's body
                    let mut pat_binds = Vec::new();
                    pat.binds(&mut pat_binds);
                    let n = pat_binds.len();
                    bound.extend(pat_binds);
                    body.walk_free(bound, out);
                    bound.truncate(bound.len() - n);
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

/// Rename every type variable in `t` with a prefix, so it lives in a
/// namespace disjoint from another type's variables. Used before unifying
/// a query against a stored (possibly polymorphic) signature so a shared
/// var name (`a` in both) doesn't capture — otherwise a legal polymorphic
/// candidate is wrongly rejected.
pub fn freshen(t: &Type, prefix: &str) -> Type {
    match t {
        Type::Named(n) => Type::Named(n.clone()),
        Type::Var(v) => Type::Var(format!("{prefix}{v}")),
        Type::App(h, args) => {
            Type::App(h.clone(), args.iter().map(|a| freshen(a, prefix)).collect())
        }
        Type::Fn(ps, r) => Type::Fn(
            ps.iter().map(|p| freshen(p, prefix)).collect(),
            Box::new(freshen(r, prefix)),
        ),
    }
}

/// Structural unification (MVP: no occurs check — callers should keep the
/// two sides' variable namespaces disjoint, e.g. via `freshen`).
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
