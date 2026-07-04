//! claw-emit-rust — transpile Claw definitions to Rust source (WS-G).
//!
//! The ecosystem-inheritance backend: any Claw module can be emitted as
//! ordinary Rust, so the outside world consumes Claw as a normal Rust
//! dependency (and Claw reaches every crate on crates.io). This avoids the
//! isolation death that killed prior "clean-slate" languages.
//!
//! Prototype scope: lowers the claw-core Expr/Type/Def surface (lambdas,
//! application, literals, references, function types) to Rust. `extern`
//! FFI declarations map a Claw name to a real Rust path. Not every Claw
//! construct lowers yet; unsupported forms are a loud error, never silent.
//!
//! Spec: docs/syntax.md §6, master-plan WS-G.

use claw_core::{Expr, Lit, Type};
use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
pub enum EmitError {
    Unsupported(String),
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmitError::Unsupported(m) => write!(f, "cannot emit Rust for: {m}"),
        }
    }
}

/// Maps Claw symbol names to the Rust path they lower to. `Nat.add` →
/// `nat_add`, or an FFI target like `sha256` → `sha2::Sha256::digest`.
pub type NameMap = BTreeMap<String, String>;

/// Lower a Claw type to a Rust type string. Type variables become generic
/// params (handled by the caller); here `Var` lowers to its name.
pub fn emit_type(t: &Type) -> String {
    match t {
        Type::Named(n) => rust_type_name(n),
        Type::Var(v) => v.to_uppercase(), // generic param, e.g. `a` -> `A`
        Type::App(head, args) => {
            let inner: Vec<String> = args.iter().map(emit_type).collect();
            match head.as_str() {
                "List" => format!("Vec<{}>", inner.join(", ")),
                "Maybe" => format!("Option<{}>", inner.join(", ")),
                "Result" => format!("Result<{}>", inner.join(", ")),
                other => format!("{}<{}>", rust_type_name(other), inner.join(", ")),
            }
        }
        Type::Fn(params, ret) => {
            let ps: Vec<String> = params.iter().map(emit_type).collect();
            format!("impl Fn({}) -> {}", ps.join(", "), emit_type(ret))
        }
    }
}

fn rust_type_name(n: &str) -> String {
    match n {
        "Nat" | "U64" => "u64".into(),
        "Int" | "I64" => "i64".into(),
        "Str" => "String".into(),
        "Bool" => "bool".into(),
        "Unit" => "()".into(),
        other => other.replace('.', "_"),
    }
}

/// Lower an expression to a Rust expression string. `names` resolves
/// referenced definitions (by rendered name) to their Rust path.
pub fn emit_expr(e: &Expr, names: &NameMap) -> Result<String, EmitError> {
    match e {
        Expr::Lit(Lit::Int(n)) => Ok(n.to_string()),
        Expr::Lit(Lit::Str(s)) => Ok(format!("{s:?}.to_string()")),
        Expr::Var(v) => Ok(sanitize_ident(v)),
        // An unresolved reference is a loud error, not a comment silently
        // emitted into expression position (which never compiles).
        Expr::Ref(h) => names
            .get(&h.0)
            .cloned()
            .ok_or_else(|| EmitError::Unsupported(format!("unresolved reference {h}"))),
        Expr::Lam { params, body } => {
            let ps: Vec<String> = params.iter().map(|p| sanitize_ident(p)).collect();
            Ok(format!("|{}| {}", ps.join(", "), emit_expr(body, names)?))
        }
        Expr::App { func, args } => {
            let f = emit_expr(func, names)?;
            let a: Result<Vec<String>, _> = args.iter().map(|x| emit_expr(x, names)).collect();
            Ok(format!("{f}({})", a?.join(", ")))
        }
        Expr::If { cond, then, els } => Ok(format!(
            "if {} {{ {} }} else {{ {} }}",
            emit_expr(cond, names)?,
            emit_expr(then, names)?,
            emit_expr(els, names)?
        )),
        Expr::Let { name, value, body } => Ok(format!(
            "{{ let {} = {}; {} }}",
            sanitize_ident(name),
            emit_expr(value, names)?,
            emit_expr(body, names)?
        )),
        Expr::Field(e, name) => Ok(format!("{}.{}", emit_expr(e, names)?, sanitize_ident(name))),
        Expr::Tag(name, args) => {
            if args.is_empty() {
                Ok(name.clone())
            } else {
                let a: Result<Vec<String>, _> = args.iter().map(|x| emit_expr(x, names)).collect();
                Ok(format!("{name}({})", a?.join(", ")))
            }
        }
        // Records need a named struct; match needs pattern translation —
        // both are honest gaps in the experimental emitter.
        Expr::Record(_) => Err(EmitError::Unsupported("record literal".into())),
        Expr::Match(_, _) => Err(EmitError::Unsupported("match expression".into())),
    }
}

// The four keywords Rust forbids as raw identifiers — they must be
// escaped by appending `_`, not `r#`.
const NON_RAW: &[&str] = &["self", "crate", "super", "Self"];

// The full Rust keyword set (2015 + 2018 + reserved). A Claw ident equal to
// any of these must be escaped or the emitted Rust won't compile.
const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while", "async", "await", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];

/// Escape a Claw identifier so it is a valid Rust identifier: dots → `_`,
/// keywords → `r#kw` (or `kw_` for the four that can't be raw).
fn sanitize_ident(v: &str) -> String {
    let base = v.replace('.', "_");
    if NON_RAW.contains(&base.as_str()) {
        format!("{base}_")
    } else if KEYWORDS.contains(&base.as_str()) {
        format!("r#{base}")
    } else {
        base
    }
}

/// Emit a `use` line for an FFI target — `extern rust "sha2" { sha256 }`.
pub fn emit_ffi_use(crate_name: &str, item: &str) -> String {
    format!("use {}::{};", crate_name.replace('-', "_"), item)
}

/// Emit a whole definition as a Rust item. A function-typed def whose body
/// is a lambda becomes a `pub fn`; anything else becomes a `pub const`.
/// Generic type variables in the signature become `<A, B, …>` params.
pub fn emit_fn(name: &str, def: &claw_core::Def, names: &NameMap) -> Result<String, EmitError> {
    let rname = name.replace('.', "_");
    match (&def.ty, &def.expr) {
        (Type::Fn(param_tys, ret), Expr::Lam { params, body })
            if params.len() == param_tys.len() =>
        {
            let generics = collect_generics(&def.ty);
            let gen = if generics.is_empty() {
                String::new()
            } else {
                format!("<{}>", generics.join(", "))
            };
            let args: Vec<String> = params
                .iter()
                .zip(param_tys)
                .map(|(p, t)| format!("{}: {}", sanitize_ident(p), emit_type(t)))
                .collect();
            Ok(format!(
                "pub fn {rname}{gen}({}) -> {} {{\n    {}\n}}",
                args.join(", "),
                emit_type(ret),
                emit_expr(body, names)?
            ))
        }
        // Function-typed value with a non-lambda body (point-free) can't be
        // a `const` — `impl Fn` is illegal in const type position. Refuse
        // loudly rather than emit non-compiling Rust.
        (Type::Fn(..), _) => Err(EmitError::Unsupported(format!(
            "point-free function `{rname}` (non-lambda body of function type)"
        ))),
        (ty, expr) => {
            // `const x: String = "..".to_string()` isn't const-evaluable;
            // emit a `&'static str` const for string values instead.
            let (ty_s, val_s) = match (ty, expr) {
                (Type::Named(n), Expr::Lit(Lit::Str(s))) if n == "Str" => {
                    ("&str".to_string(), format!("{s:?}"))
                }
                _ => (emit_type(ty), emit_expr(expr, names)?),
            };
            Ok(format!("pub const {rname}: {ty_s} = {val_s};"))
        }
    }
}

/// Distinct type variables in a type, uppercased for Rust generics.
fn collect_generics(t: &Type) -> Vec<String> {
    let mut out = Vec::new();
    walk_generics(t, &mut out);
    out.sort();
    out.dedup();
    out
}

fn walk_generics(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::Var(v) => out.push(v.to_uppercase()),
        Type::Named(_) => {}
        Type::App(_, args) => args.iter().for_each(|a| walk_generics(a, out)),
        Type::Fn(ps, r) => {
            ps.iter().for_each(|p| walk_generics(p, out));
            walk_generics(r, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::Def;

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    #[test]
    fn primitive_types_lower_to_rust() {
        assert_eq!(emit_type(&named("Nat")), "u64");
        assert_eq!(emit_type(&named("Str")), "String");
        assert_eq!(emit_type(&named("Bool")), "bool");
    }

    #[test]
    fn containers_lower() {
        let list_nat = Type::App("List".into(), vec![named("Nat")]);
        assert_eq!(emit_type(&list_nat), "Vec<u64>");
        let maybe = Type::App("Maybe".into(), vec![named("Str")]);
        assert_eq!(emit_type(&maybe), "Option<String>");
    }

    #[test]
    fn lambda_lowers_to_closure() {
        // \a, b -> a
        let e = Expr::Lam {
            params: vec!["a".into(), "b".into()],
            body: Box::new(Expr::Var("a".into())),
        };
        assert_eq!(emit_expr(&e, &NameMap::new()).unwrap(), "|a, b| a");
    }

    #[test]
    fn application_lowers_and_resolves_refs() {
        let dep = Def::new(Expr::Lit(Lit::Int(0)), named("Nat"));
        let h = dep.hash();
        let mut names = NameMap::new();
        names.insert(h.0.clone(), "nat_add".into());

        let e = Expr::App {
            func: Box::new(Expr::Ref(h)),
            args: vec![Expr::Lit(Lit::Int(2)), Expr::Lit(Lit::Int(3))],
        };
        assert_eq!(emit_expr(&e, &names).unwrap(), "nat_add(2, 3)");
    }

    #[test]
    fn reserved_words_are_escaped() {
        let e = Expr::Var("fn".into());
        assert_eq!(emit_expr(&e, &NameMap::new()).unwrap(), "r#fn");
    }

    #[test]
    fn ffi_use_line() {
        assert_eq!(emit_ffi_use("sha2", "Sha256"), "use sha2::Sha256;");
    }

    #[test]
    fn emit_fn_lowers_a_function_def() {
        // double : Nat -> Nat = \p0 -> Nat.add p0 p0
        let def = Def::new(
            Expr::Lam {
                params: vec!["p0".into()],
                body: Box::new(Expr::App {
                    func: Box::new(Expr::Var("Nat.add".into())),
                    args: vec![Expr::Var("p0".into()), Expr::Var("p0".into())],
                }),
            },
            Type::Fn(vec![named("Nat")], Box::new(named("Nat"))),
        );
        let out = emit_fn("double", &def, &NameMap::new()).unwrap();
        assert_eq!(
            out,
            "pub fn double(p0: u64) -> u64 {\n    Nat_add(p0, p0)\n}"
        );
    }

    #[test]
    fn reserved_words_escape_correctly() {
        // `self` can't be raw (→ self_); `struct` can (→ r#struct); `move` too.
        assert_eq!(sanitize_ident("self"), "self_");
        assert_eq!(sanitize_ident("crate"), "crate_");
        assert_eq!(sanitize_ident("struct"), "r#struct");
        assert_eq!(sanitize_ident("return"), "r#return");
        assert_eq!(sanitize_ident("ok"), "ok"); // ordinary ident untouched
    }

    #[test]
    fn unresolved_ref_is_a_loud_error() {
        let e = Expr::App {
            func: Box::new(Expr::Ref(claw_core::Hash("deadbeef".into()))),
            args: vec![],
        };
        assert!(matches!(
            emit_expr(&e, &NameMap::new()),
            Err(EmitError::Unsupported(_))
        ));
    }

    #[test]
    fn emit_fn_adds_generics_for_type_vars() {
        // id : a -> a = \p0 -> p0
        let def = Def::new(
            Expr::Lam {
                params: vec!["p0".into()],
                body: Box::new(Expr::Var("p0".into())),
            },
            Type::Fn(vec![Type::Var("a".into())], Box::new(Type::Var("a".into()))),
        );
        let out = emit_fn("id", &def, &NameMap::new()).unwrap();
        assert!(out.starts_with("pub fn id<A>(p0: A) -> A"), "got: {out}");
    }

    #[test]
    fn double_end_to_end() {
        // double = \x -> nat_add x x   →   |x| nat_add(x, x)
        let dep = Def::new(Expr::Lit(Lit::Int(0)), named("Nat"));
        let h = dep.hash();
        let mut names = NameMap::new();
        names.insert(h.0.clone(), "nat_add".into());
        let double = Expr::Lam {
            params: vec!["x".into()],
            body: Box::new(Expr::App {
                func: Box::new(Expr::Ref(h)),
                args: vec![Expr::Var("x".into()), Expr::Var("x".into())],
            }),
        };
        assert_eq!(emit_expr(&double, &names).unwrap(), "|x| nat_add(x, x)");
    }
}
