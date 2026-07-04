//! Render definitions to `.claw` source text — the human-readable
//! projection the code-as-database promises. The CDB stores content-
//! addressed AST; this is how a person (or a diff, or `clawc`) sees it.
//!
//! Inverse-ish of `ingest`: CDB → text, where ingest is text → CDB. The
//! surface follows Roc/Claw conventions (`name : Type` then
//! `name = |params| body`, application as `f(a, b)`).

use crate::{Def, Expr, Lit};

/// Render a named definition as a `.claw` declaration:
/// ```text
/// double : Nat -> Nat
/// double = |p0| Nat.add(p0, p0)
/// ```
pub fn render_def(name: &str, def: &Def) -> String {
    let sig = format!("{name} : {}", def.ty);
    let body = render_expr(&def.expr);
    format!("{sig}\n{name} = {body}")
}

/// Render an expression to `.claw` surface syntax.
pub fn render_expr(e: &Expr) -> String {
    match e {
        Expr::Var(v) => v.clone(),
        Expr::Ref(h) => format!("{h}"), // by hash — only in raw CDB dumps
        Expr::Lit(Lit::Int(n)) => n.to_string(),
        Expr::Lit(Lit::Str(s)) => format!("{s:?}"),
        Expr::Lam { params, body } => {
            format!("|{}| {}", params.join(", "), render_expr(body))
        }
        Expr::App { func, args } => {
            let a: Vec<String> = args.iter().map(render_expr).collect();
            // A lambda in function position must be parenthesized, else
            // `(|x| x)(5)` renders as `|x| x(5)` — a different tree.
            let head = match &**func {
                Expr::Lam { .. } => format!("({})", render_expr(func)),
                _ => render_expr(func),
            };
            format!("{}({})", head, a.join(", "))
        }
        Expr::If { cond, then, els } => format!(
            "if {} {} else {}",
            render_expr(cond),
            render_expr(then),
            render_expr(els)
        ),
        Expr::Let { name, value, body } => {
            format!("{name} = {}\n{}", render_expr(value), render_expr(body))
        }
    }
}

/// Render a whole module from (name, def) pairs.
pub fn render_module(defs: &[(String, Def)]) -> String {
    defs.iter()
        .map(|(n, d)| render_def(n, d))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Type;

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    #[test]
    fn renders_a_function_def() {
        // double = |p0| Nat.add(p0, p0)
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
        let out = render_def("double", &def);
        assert_eq!(out, "double : Nat -> Nat\ndouble = |p0| Nat.add(p0, p0)");
    }

    #[test]
    fn renders_literals_and_nesting() {
        let e = Expr::App {
            func: Box::new(Expr::Var("Nat.add".into())),
            args: vec![
                Expr::Lit(Lit::Int(2)),
                Expr::App {
                    func: Box::new(Expr::Var("Nat.mul".into())),
                    args: vec![Expr::Var("p0".into()), Expr::Lit(Lit::Int(3))],
                },
            ],
        };
        assert_eq!(render_expr(&e), "Nat.add(2, Nat.mul(p0, 3))");
    }

    #[test]
    fn module_joins_defs() {
        let a = Def::new(Expr::Lit(Lit::Int(1)), named("Nat"));
        let b = Def::new(Expr::Lit(Lit::Int(2)), named("Nat"));
        let m = render_module(&[("one".into(), a), ("two".into(), b)]);
        assert!(m.contains("one : Nat\none = 1"));
        assert!(m.contains("two : Nat\ntwo = 2"));
    }
}
