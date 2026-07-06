//! Real-compiler compile signal: render a task's scope + the produced
//! definitions as an actual `.achuk` module and run `achukc check` on it.
//!
//! The grader's built-in `compiled` predicate is a fast proxy ("no dangling
//! references"). This module is the ground truth: the vendored compiler's
//! full parse + canonicalize + typecheck. Scope symbols become signature-
//! true stubs whose bodies are `crash` blocks (`crash` types as `*`, so a
//! stub typechecks at any signature without implementing semantics), and
//! CDB names are mangled to legal surface identifiers (`Nat.add` →
//! `nat_add`, `File.read!` → `file_read`) in both the stubs and the
//! produced expressions, so what achukc sees is one closed, honest module.
//!
//! achukc's exit code is not a reliable gate (warnings can flip it); the
//! signal is the `Found N error(s)` line it always prints.

use crate::{ProducedDef, ScopeEntry};
use achuk_core::render::render_expr;
use achuk_core::{Expr, Type};
use std::path::PathBuf;

/// Mangle a CDB symbol name to a legal `.achuk` value identifier.
pub fn mangle(name: &str) -> String {
    let mut s = name.replace('.', "_").replace('!', "");
    s = s.to_lowercase();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// Map the benchmark's fictional type names onto the compiler's real ones.
fn map_ty(t: &Type) -> Type {
    match t {
        Type::Named(n) => Type::Named(match n.as_str() {
            "Nat" => "U64".into(),
            "Unit" => "{}".into(),
            other => other.to_string(),
        }),
        Type::Var(v) => Type::Var(v.clone()),
        Type::App(head, args) => Type::App(head.clone(), args.iter().map(map_ty).collect()),
        Type::Fn(ps, r) => Type::Fn(ps.iter().map(map_ty).collect(), Box::new(map_ty(r))),
    }
}

/// Rewrite scope references (dotted / bang names) to their mangled forms.
/// Parameters (`p0`…) and sibling definition names pass through untouched.
fn map_expr(e: &Expr) -> Expr {
    match e {
        Expr::Var(v) if v.contains('.') || v.contains('!') => Expr::Var(mangle(v)),
        Expr::Var(v) => Expr::Var(v.clone()),
        Expr::Ref(h) => Expr::Ref(h.clone()),
        Expr::Lit(l) => Expr::Lit(l.clone()),
        Expr::Lam { params, body } => Expr::Lam {
            params: params.clone(),
            body: Box::new(map_expr(body)),
        },
        Expr::App { func, args } => Expr::App {
            func: Box::new(map_expr(func)),
            args: args.iter().map(map_expr).collect(),
        },
        Expr::If { cond, then, els } => Expr::If {
            cond: Box::new(map_expr(cond)),
            then: Box::new(map_expr(then)),
            els: Box::new(map_expr(els)),
        },
        Expr::Let { name, value, body } => Expr::Let {
            name: name.clone(),
            value: Box::new(map_expr(value)),
            body: Box::new(map_expr(body)),
        },
        Expr::Record(fields) => {
            Expr::Record(fields.iter().map(|(n, e)| (n.clone(), map_expr(e))).collect())
        }
        Expr::Field(e, n) => Expr::Field(Box::new(map_expr(e)), n.clone()),
        Expr::Tag(n, args) => Expr::Tag(n.clone(), args.iter().map(map_expr).collect()),
        Expr::Match(s, arms) => Expr::Match(
            Box::new(map_expr(s)),
            arms.iter().map(|(p, b)| (p.clone(), map_expr(b))).collect(),
        ),
    }
}

/// Render a stub for a scope symbol: true signature, `crash` body.
fn render_stub(entry_name: &str, ty: &Type) -> String {
    let name = mangle(entry_name);
    let arity = match ty {
        Type::Fn(ps, _) => ps.len(),
        _ => 0,
    };
    let body = if arity == 0 {
        "{ crash \"stub\" }".to_string()
    } else {
        let params: Vec<String> = (0..arity).map(|i| format!("_s{i}")).collect();
        format!("|{}| {{ crash \"stub\" }}", params.join(", "))
    };
    format!("{name} : {}\n{name} = {body}", map_ty(ty))
}

/// Build the complete `.achuk` module for a (scope, produced) pair.
pub fn to_module(scope: &[(String, Type)], produced: &[ProducedDef]) -> String {
    let produced_names: Vec<String> = produced
        .iter()
        .enumerate()
        .map(|(i, pd)| {
            pd.name
                .clone()
                .map(|n| mangle(&n))
                .unwrap_or_else(|| format!("produced_{i}"))
        })
        .collect();

    let mut parts = vec![format!("module [{}]", produced_names.join(", "))];
    for (name, ty) in scope {
        // A produced def that reuses a scope name shadows the stub — emit
        // only the produced version (duplicate definitions are an error).
        if produced_names.iter().any(|p| *p == mangle(name)) {
            continue;
        }
        parts.push(render_stub(name, ty));
    }
    for (pd, name) in produced.iter().zip(&produced_names) {
        let ty = map_ty(&pd.def.ty);
        let body = render_expr(&map_expr(&pd.def.expr));
        parts.push(format!("{name} : {ty}\n{name} = {body}"));
    }
    parts.join("\n\n")
}

/// Convenience: build the module straight from `ScopeEntry`s (parsing each
/// signature with the same parser the grader uses).
pub fn task_module(scope: &[ScopeEntry], produced: &[ProducedDef]) -> anyhow::Result<String> {
    let mut pairs = Vec::with_capacity(scope.len());
    for e in scope {
        let ty = achuk_core::parse::parse_type(&e.ty)
            .map_err(|err| anyhow::anyhow!("scope `{}`: {err}", e.name))?;
        pairs.push((e.name.clone(), ty));
    }
    Ok(to_module(&pairs, produced))
}

/// Locate achukc: `$ACHUK_COMPILER`, else `achukc` on PATH.
fn find_achukc() -> PathBuf {
    std::env::var("ACHUK_COMPILER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("achukc"))
}

/// The verdict of a real `achukc check` run.
#[derive(Debug, Clone)]
pub struct RealCheck {
    pub compiled: bool,
    pub errors: u32,
    /// First few error lines, for diagnostics.
    pub detail: String,
}

/// Run `achukc check` on a module. The signal is the `Found N error(s)`
/// summary line, not the exit code.
pub fn achukc_check(module_src: &str) -> anyhow::Result<RealCheck> {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "achuk-realc-{}-{}.achuk",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, module_src)?;
    let out = std::process::Command::new(find_achukc())
        .arg("check")
        .arg(&tmp)
        .output()
        .map_err(|e| anyhow::anyhow!("running achukc: {e} (set ACHUK_COMPILER)"))?;
    let _ = std::fs::remove_file(&tmp);

    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let errors = text
        .lines()
        .rev()
        .find_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("Found ")?;
            let (n, tail) = rest.split_once(' ')?;
            tail.starts_with("error").then(|| n.parse::<u32>().ok())?
        })
        .ok_or_else(|| anyhow::anyhow!("achukc produced no 'Found N error(s)' summary:\n{text}"))?;

    // achukc prints each diagnostic as its own box separated by blank lines.
    // Our verify wrapper adds a `module [...]` header, which triggers a
    // harmless "MODULE HEADER DEPRECATED" warning — drop that block so the
    // user sees only the real errors, not self-inflicted noise.
    // The verify wrapper's `module [...]` header always makes the FIRST
    // diagnostic box a harmless "MODULE HEADER DEPRECATED" warning. Skip the
    // whole first box (up to the second box top `┌`) so the user sees the
    // real error, not the ASCII-boxed deprecation notice.
    let lines: Vec<&str> = text.lines().collect();
    let box_tops: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains('\u{250c}')) // ┌
        .map(|(i, _)| i)
        .collect();
    let start = if box_tops.len() >= 2
        && lines[box_tops[0]..box_tops[1]]
            .iter()
            .any(|l| l.to_uppercase().contains("DEPRECATED"))
    {
        box_tops[1]
    } else {
        0
    };
    let detail = lines[start..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .take(8)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    Ok(RealCheck {
        compiled: errors == 0,
        errors,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangles_scope_names() {
        assert_eq!(mangle("Nat.add"), "nat_add");
        assert_eq!(mangle("File.read!"), "file_read");
        assert_eq!(mangle("Stdout.line!"), "stdout_line");
    }

    #[test]
    fn module_renders_stubs_and_produced() {
        use achuk_core::{Def, Expr};
        let scope = vec![(
            "Nat.max".to_string(),
            achuk_core::parse::parse_type("Nat, Nat -> Nat").unwrap(),
        )];
        let def = Def::new(
            Expr::Lam {
                params: vec!["p0".into()],
                body: Box::new(Expr::App {
                    func: Box::new(Expr::Var("Nat.max".into())),
                    args: vec![Expr::Var("p0".into()), Expr::Lit(achuk_core::Lit::Int(6))],
                }),
            },
            achuk_core::parse::parse_type("Nat -> Nat").unwrap(),
        );
        let m = to_module(
            &scope,
            &[ProducedDef {
                name: Some("floor_at_6".into()),
                def,
            }],
        );
        assert!(m.starts_with("module [floor_at_6]"));
        assert!(m.contains("nat_max : U64, U64 -> U64"));
        assert!(m.contains("crash \"stub\""));
        assert!(m.contains("floor_at_6 = |p0| nat_max(p0, 6)"));
    }
}
