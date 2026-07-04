//! GBNF projection: turn a `Mask` into a llama.cpp-style grammar that
//! constrains decode to the Def-JSON output protocol, with symbol
//! references restricted to the mask's legal continuations.
//!
//! The guarantee, at the token level: the model **cannot emit an
//! uppercase/dotted symbol name that is not in the mask** — the library-API
//! hallucination class (`generate_nonce()`, `Nonce.generate`) becomes
//! ungeneratable. Lowercase local identifiers (lambda params) stay free;
//! unbound locals are caught by the grader, since a context-free grammar
//! cannot track binding structure.
//!
//! `Expr::Ref` is deliberately absent from the grammar: models reference
//! symbols by name; hashes are agent-tooling. A model under this grammar
//! cannot emit a dangling hash at all.

use crate::Continuation;

/// Escape a symbol name for inclusion in a GBNF string literal.
fn escape(name: &str) -> String {
    name.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build the full Def-JSON grammar. `mask` = the legal continuations for
/// this generation (from `legal_continuations`); empty mask ⇒ the model
/// can only reference its own lambda parameters.
pub fn def_json_grammar(mask: &[Continuation]) -> String {
    let mut g = String::new();

    g.push_str("root ::= ws \"[\" ws def (ws \",\" ws def)* ws \"]\" ws\n");
    g.push_str(
        "def ::= \"{\" ws \"\\\"expr\\\"\" ws \":\" ws expr ws \",\" ws \"\\\"ty\\\"\" ws \":\" ws type ws \",\" ws \"\\\"effects\\\"\" ws \":\" ws \"[]\" ws \",\" ws \"\\\"deprecated\\\"\" ws \":\" ws \"false\" ws \",\" ws \"\\\"doc\\\"\" ws \":\" ws \"\\\"\\\"\" ws \"}\"\n",
    );
    // Depth-bounded expression rules. `expr` is recursive (App/Lam nest
    // exprs), so without a bound a weak decoder can nest App forever until
    // it hits max_tokens. Leveling expr0..exprN, where exprN is a leaf
    // (no App/Lam), makes the grammar finite-depth and forces termination.
    const MAX_DEPTH: usize = 6;
    g.push_str("expr ::= expr0\n");
    for k in 0..MAX_DEPTH {
        g.push_str(&format!("expr{k} ::= evar | elit | elam{k} | eapp{k}\n"));
        g.push_str(&format!(
            "elam{k} ::= \"{{\" ws \"\\\"Lam\\\"\" ws \":\" ws \"{{\" ws \"\\\"params\\\"\" ws \":\" ws \"[\" ws (paramname (ws \",\" ws paramname)*)? ws \"]\" ws \",\" ws \"\\\"body\\\"\" ws \":\" ws expr{next} ws \"}}\" ws \"}}\"\n",
            next = k + 1
        ));
        g.push_str(&format!(
            "eapp{k} ::= \"{{\" ws \"\\\"App\\\"\" ws \":\" ws \"{{\" ws \"\\\"func\\\"\" ws \":\" ws expr{next} ws \",\" ws \"\\\"args\\\"\" ws \":\" ws \"[\" ws (expr{next} (ws \",\" ws expr{next})*)? ws \"]\" ws \"}}\" ws \"}}\"\n",
            next = k + 1
        ));
    }
    // Leaf level: no further nesting.
    g.push_str(&format!("expr{MAX_DEPTH} ::= evar | elit\n"));
    g.push_str("evar ::= \"{\" ws \"\\\"Var\\\"\" ws \":\" ws varname ws \"}\"\n");

    // The load-bearing rule: symbol names are an explicit alternation.
    if mask.is_empty() {
        g.push_str("varname ::= paramname\n");
    } else {
        let alts: Vec<String> = mask
            .iter()
            .map(|c| format!("\"\\\"{}\\\"\"", escape(&c.name)))
            .collect();
        g.push_str(&format!("scopename ::= {}\n", alts.join(" | ")));
        g.push_str("varname ::= scopename | paramname\n");
    }

    g.push_str("paramname ::= \"\\\"\" [a-z] [a-z0-9_]* \"\\\"\"\n");
    g.push_str(
        "elit ::= \"{\" ws \"\\\"Lit\\\"\" ws \":\" ws (\"{\" ws \"\\\"Int\\\"\" ws \":\" ws int ws \"}\" | \"{\" ws \"\\\"Str\\\"\" ws \":\" ws string ws \"}\") ws \"}\"\n",
    );
    g.push_str("int ::= \"-\"? [0-9]+\n");
    g.push_str("string ::= \"\\\"\" [^\"\\\\]* \"\\\"\"\n");
    g.push_str("type ::= tnamed | tvar | tapp | tfn\n");
    g.push_str("tnamed ::= \"{\" ws \"\\\"Named\\\"\" ws \":\" ws tyname ws \"}\"\n");
    g.push_str("tvar ::= \"{\" ws \"\\\"Var\\\"\" ws \":\" ws paramname ws \"}\"\n");
    g.push_str(
        "tapp ::= \"{\" ws \"\\\"App\\\"\" ws \":\" ws \"[\" ws tyname ws \",\" ws \"[\" ws (type (ws \",\" ws type)*)? ws \"]\" ws \"]\" ws \"}\"\n",
    );
    g.push_str(
        "tfn ::= \"{\" ws \"\\\"Fn\\\"\" ws \":\" ws \"[\" ws \"[\" ws (type (ws \",\" ws type)*)? ws \"]\" ws \",\" ws type ws \"]\" ws \"}\"\n",
    );
    g.push_str("tyname ::= \"\\\"\" [A-Z] [a-zA-Z0-9_.]* \"\\\"\"\n");
    g.push_str("ws ::= [ \\t\\n]*\n");
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::{Hash, Type};
    use std::collections::BTreeMap;

    fn cont(name: &str) -> Continuation {
        Continuation {
            name: name.into(),
            hash: Hash("00".repeat(32)),
            ty: Type::Named("T".into()),
            subst: BTreeMap::new(),
        }
    }

    #[test]
    fn scope_names_become_explicit_alternation() {
        let g = def_json_grammar(&[cont("Nat.add"), cont("Nat.checkedSub")]);
        assert!(g.contains(r#"scopename ::= "\"Nat.add\"" | "\"Nat.checkedSub\"""#));
        assert!(g.contains("varname ::= scopename | paramname"));
    }

    #[test]
    fn empty_mask_allows_only_params() {
        let g = def_json_grammar(&[]);
        assert!(!g.contains("scopename"));
        assert!(g.contains("varname ::= paramname\n"));
    }

    #[test]
    fn hallucination_target_not_in_grammar() {
        // The point of it all: `generate_nonce` is nowhere in the grammar,
        // so a grammar-constrained decoder cannot emit it as a scope symbol.
        let g = def_json_grammar(&[cont("Nat.add")]);
        assert!(!g.contains("generate_nonce"));
    }

    #[test]
    fn expr_recursion_is_depth_bounded() {
        // The leaf level must not recurse into App/Lam — otherwise a weak
        // decoder can nest forever until max_tokens (observed with 0.5B).
        let g = def_json_grammar(&[cont("Nat.add")]);
        assert!(
            g.contains("expr6 ::= evar | elit"),
            "leaf level must be non-recursive"
        );
        assert!(
            !g.contains("expr6 ::= evar | elit | elam"),
            "leaf must not nest"
        );
        assert!(g.contains("expr0 ::= evar | elit | elam0 | eapp0"));
    }

    #[test]
    fn names_are_escaped() {
        let g = def_json_grammar(&[cont(r#"weird"name"#)]);
        assert!(g.contains(r#"weird\"name"#));
    }

    #[test]
    fn grammar_has_all_structural_rules() {
        let g = def_json_grammar(&[cont("X.y")]);
        for rule in [
            "root ::=",
            "def ::=",
            "expr ::= expr0",
            "expr0 ::=",
            "elam0 ::=",
            "eapp0 ::=",
            "type ::=",
            "ws ::=",
        ] {
            assert!(g.contains(rule), "missing rule {rule}");
        }
        // Ref is not a generatable form
        assert!(!g.contains("Ref"));
    }
}
