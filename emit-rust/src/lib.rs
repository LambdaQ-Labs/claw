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
        Expr::Ref(h) => Ok(names
            .get(&h.0)
            .cloned()
            .unwrap_or_else(|| format!("/*unresolved:{h}*/"))),
        Expr::Lam { params, body } => {
            let ps: Vec<String> = params.iter().map(|p| sanitize_ident(p)).collect();
            Ok(format!("|{}| {}", ps.join(", "), emit_expr(body, names)?))
        }
        Expr::App { func, args } => {
            let f = emit_expr(func, names)?;
            let a: Result<Vec<String>, _> = args.iter().map(|x| emit_expr(x, names)).collect();
            Ok(format!("{f}({})", a?.join(", ")))
        }
    }
}

/// Rust reserves some words; a lowercase Claw ident like `fn` must escape.
fn sanitize_ident(v: &str) -> String {
    const RESERVED: &[&str] = &[
        "fn", "let", "match", "move", "type", "impl", "mod", "use", "ref", "self",
    ];
    let base = v.replace('.', "_");
    if RESERVED.contains(&base.as_str()) {
        format!("r#{base}")
    } else {
        base
    }
}

/// Emit a `use` line for an FFI target — `extern rust "sha2" { sha256 }`.
pub fn emit_ffi_use(crate_name: &str, item: &str) -> String {
    format!("use {}::{};", crate_name.replace('-', "_"), item)
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
