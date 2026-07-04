//! claw-bench-grader — the deterministic grader (WS-J).
//!
//! Grading is a pure function of (task, produced state). No model in the
//! loop, reproducible, CI-runnable. Multi-signal: compile-shaped checks ∧
//! hallucination detection ∧ forbidden rules. Tests/contract execution
//! plug in as the compiler comes online — the schema carries them now.
//!
//! Spec: docs/benchmark-harness.md §3.

mod exec;

use claw_cdb::Cdb;
use claw_core::Def;
use serde::{Deserialize, Serialize};

pub use exec::{run_contracts, ContractRun};

/// A definition the model produced, optionally naming itself. The name
/// lets a def reference itself (recursion) or its siblings without that
/// being counted as a hallucination — they are being defined right here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProducedDef {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub def: Def,
}

// ---------------------------------------------------------------------
// Task schema (docs/benchmark-harness.md §2.2)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    FromScratch,
    Translate,
    RepoFeature,
    Contract,
    Effect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeSpec {
    /// Must the produced code typecheck?
    #[serde(default = "default_true")]
    pub compile: bool,
    /// Test oracles (paths to .claw test specs; executed once the compiler lands).
    #[serde(default)]
    pub tests: Vec<String>,
    /// Preconditions — filter the generated input cases during execution.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Contract assertions (postconditions) that must hold.
    #[serde(default)]
    pub contracts: Vec<String>,
    /// Rules the produced code must not trip (e.g. "hallucinated-symbol").
    #[serde(default)]
    pub forbidden: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// A symbol seeded into the CDB before the task runs — the task's
/// "repository context". Types use the signature syntax
/// (`claw_core::parse::parse_type`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeEntry {
    pub name: String,
    pub ty: String,
    #[serde(default)]
    pub deprecated: bool,
}

/// A named scalar parameter of the function under test, in signature
/// order. When present, the grader can *execute* the produced definition
/// on generated inputs and check contracts against real results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    #[serde(default = "default_nat")]
    pub ty: String,
}

fn default_nat() -> String {
    "Nat".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub category: Category,
    pub prompt: String,
    /// In-scope symbols available to the solution (the CDB snapshot).
    #[serde(default)]
    pub scope: Vec<ScopeEntry>,
    /// Scalar parameters (in order) enabling contract *execution*.
    #[serde(default)]
    pub params: Vec<Param>,
    pub grade: GradeSpec,
    /// Path to the reference solution (not shown to the model).
    #[serde(default)]
    pub reference: Option<String>,
}

impl Task {
    /// Build the CDB this task runs against: one definition per scope
    /// entry. Placeholder bodies (unique per name) — `candidates()` and
    /// hallucination detection only need names, types, and hashes.
    pub fn build_scope_cdb(&self) -> anyhow::Result<Cdb> {
        use claw_core::{parse::parse_type, Expr, Lit};
        let mut cdb = Cdb::in_memory()?;
        for entry in &self.scope {
            let ty = parse_type(&entry.ty)
                .map_err(|e| anyhow::anyhow!("scope `{}`: {e}", entry.name))?;
            let mut def = Def::new(Expr::Lit(Lit::Str(entry.name.clone())), ty);
            def.deprecated = entry.deprecated;
            let h = cdb.put(&def)?;
            cdb.bind(&entry.name, &h)?;
        }
        Ok(cdb)
    }
}

// ---------------------------------------------------------------------
// Grade result (docs/benchmark-harness.md §3)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeResult {
    pub task_id: String,
    pub compiled: bool,
    pub tests_passed: (u32, u32),
    pub contracts_held: (u32, u32),
    pub forbidden_hit: Vec<String>,
    /// Symbols the produced code references that do not exist in the CDB —
    /// the headline metric the constraint server must drive to ~0.
    pub hallucinated_symbols: Vec<String>,
    /// Effects the produced code performs but does not declare (WS-F).
    #[serde(default)]
    pub effect_unsound: Vec<String>,
    pub pass: bool,
    pub retries_used: u32,
    pub tokens: u64,
}

/// Grade produced definitions against a task, in the context of a CDB.
///
/// Hallucination detection: (a) any `Ref(hash)` the CDB doesn't contain,
/// (b) any free variable that resolves neither to a CDB symbol nor to a
/// name the model is defining in this same batch (so recursion and
/// mutual reference are legal, not hallucinations).
pub fn grade(
    task: &Task,
    produced: &[ProducedDef],
    cdb: &Cdb,
    retries_used: u32,
    tokens: u64,
) -> anyhow::Result<GradeResult> {
    let mut hallucinated: Vec<String> = Vec::new();

    let mut known_names: std::collections::BTreeSet<String> =
        cdb.symbols()?.into_iter().map(|(n, _)| n).collect();
    // The names being defined right now are in scope for each other.
    for pd in produced {
        if let Some(n) = &pd.name {
            known_names.insert(n.clone());
        }
    }

    for pd in produced {
        for h in pd.def.deps() {
            if !cdb.contains_hash(&h)? {
                hallucinated.push(format!("ref:{h}"));
            }
        }
        for v in pd.def.expr.free_vars() {
            if !known_names.contains(&v) {
                hallucinated.push(format!("name:{v}"));
            }
        }
    }
    hallucinated.sort();
    hallucinated.dedup();

    // "Compiled" for the prototype = no dangling references. The real
    // typecheck replaces this predicate when the compiler comes online;
    // the interface stays fixed.
    let compiled = hallucinated.is_empty();

    let mut forbidden_hit = Vec::new();
    if task
        .grade
        .forbidden
        .iter()
        .any(|f| f == "hallucinated-symbol")
        && !hallucinated.is_empty()
    {
        forbidden_hit.push("hallucinated-symbol".to_string());
    }

    // Test oracles still need the compiler's test runner — reported 0/N,
    // never silently passed (docs/benchmark-harness.md §7).
    let tests_total = task.grade.tests.len() as u32;
    let tests_passed = (0, tests_total);

    // Contracts: EXECUTED when the task declares scalar params and the
    // produced code runs (WS-E, wired). Otherwise 0/N (ungraded, honest).
    let contracts_total = task.grade.contracts.len() as u32;
    let scope_names: Vec<String> = cdb.symbols()?.into_iter().map(|(n, _)| n).collect();
    let contracts_held = if compiled {
        match exec::run_contracts(
            produced,
            &task.params,
            &task.grade.requires,
            &task.grade.contracts,
            &scope_names,
        ) {
            exec::ContractRun::Checked(held, total) => (held, total),
            exec::ContractRun::Skipped => (0, contracts_total),
        }
    } else {
        (0, contracts_total)
    };

    // Effect soundness (WS-F): does declared cover what the code performs?
    let mut effect_unsound: Vec<String> = Vec::new();
    for pd in produced {
        if let Ok(chk) = claw_effects::check_by_names(cdb, &pd.def) {
            effect_unsound.extend(chk.undeclared);
        }
    }
    effect_unsound.sort();
    effect_unsound.dedup();
    if task.grade.forbidden.iter().any(|f| f == "effect-unsound") && !effect_unsound.is_empty() {
        forbidden_hit.push("effect-unsound".to_string());
    }

    let pass = (!task.grade.compile || compiled)
        && tests_passed.0 == tests_total
        && contracts_held.0 == contracts_total
        && forbidden_hit.is_empty();

    Ok(GradeResult {
        task_id: task.id.clone(),
        compiled,
        tests_passed,
        contracts_held,
        forbidden_hit,
        hallucinated_symbols: hallucinated,
        effect_unsound,
        pass,
        retries_used,
        tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::{Expr, Hash, Lit, Type};

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    fn pd(def: Def) -> ProducedDef {
        ProducedDef { name: None, def }
    }

    fn pd_named(name: &str, def: Def) -> ProducedDef {
        ProducedDef {
            name: Some(name.into()),
            def,
        }
    }

    fn simple_task(forbidden: Vec<String>) -> Task {
        Task {
            id: "t-001".into(),
            category: Category::FromScratch,
            prompt: "produce a Nat".into(),
            scope: vec![],
            params: vec![],
            grade: GradeSpec {
                compile: true,
                requires: vec![],
                tests: vec![],
                contracts: vec![],
                forbidden,
            },
            reference: None,
        }
    }

    #[test]
    fn clean_production_passes() {
        let mut cdb = Cdb::in_memory().unwrap();
        let dep = Def::new(Expr::Lit(Lit::Int(1)), named("Nat"));
        let dep_h = cdb.put(&dep).unwrap();
        cdb.bind("one", &dep_h).unwrap();

        let produced = Def::new(
            Expr::App {
                func: Box::new(Expr::Ref(dep_h)),
                args: vec![],
            },
            named("Nat"),
        );
        let r = grade(&simple_task(vec![]), &[pd(produced)], &cdb, 0, 100).unwrap();
        assert!(r.compiled);
        assert!(r.hallucinated_symbols.is_empty());
        assert!(r.pass);
    }

    #[test]
    fn dangling_ref_is_hallucination_and_fails() {
        let cdb = Cdb::in_memory().unwrap();
        let ghost = Hash("ab".repeat(32));
        let produced = Def::new(
            Expr::App {
                func: Box::new(Expr::Ref(ghost)),
                args: vec![],
            },
            named("Nat"),
        );
        let r = grade(
            &simple_task(vec!["hallucinated-symbol".into()]),
            &[pd(produced)],
            &cdb,
            2,
            500,
        )
        .unwrap();
        assert!(!r.compiled);
        assert_eq!(r.hallucinated_symbols.len(), 1);
        assert!(r.hallucinated_symbols[0].starts_with("ref:"));
        assert_eq!(r.forbidden_hit, vec!["hallucinated-symbol"]);
        assert!(!r.pass);
    }

    #[test]
    fn unresolved_free_name_is_hallucination() {
        // the `generate_nonce()` case: a name nothing binds
        let cdb = Cdb::in_memory().unwrap();
        let produced = Def::new(
            Expr::App {
                func: Box::new(Expr::Var("generate_nonce".into())),
                args: vec![],
            },
            named("Bytes"),
        );
        let r = grade(&simple_task(vec![]), &[pd(produced)], &cdb, 0, 50).unwrap();
        assert_eq!(r.hallucinated_symbols, vec!["name:generate_nonce"]);
        assert!(!r.pass);
    }

    #[test]
    fn self_recursion_is_not_a_hallucination() {
        // fac = \n -> ... fac ...  — a named def referencing itself is legal.
        let cdb = Cdb::in_memory().unwrap();
        let body = Def::new(
            Expr::Lam {
                params: vec!["n".into()],
                body: Box::new(Expr::App {
                    func: Box::new(Expr::Var("fac".into())),
                    args: vec![Expr::Var("n".into())],
                }),
            },
            Type::Fn(vec![named("Nat")], Box::new(named("Nat"))),
        );
        let r = grade(
            &simple_task(vec!["hallucinated-symbol".into()]),
            &[pd_named("fac", body)],
            &cdb,
            0,
            50,
        )
        .unwrap();
        assert!(
            r.hallucinated_symbols.is_empty(),
            "self-reference must be in scope"
        );
        assert!(r.compiled);
    }

    #[test]
    fn mutual_recursion_across_batch_is_legal() {
        // isEven references isOdd and vice-versa — both defined in the batch.
        let cdb = Cdb::in_memory().unwrap();
        let mk = |other: &str| {
            Def::new(
                Expr::App {
                    func: Box::new(Expr::Var(other.into())),
                    args: vec![],
                },
                named("Bool"),
            )
        };
        let batch = [
            pd_named("isEven", mk("isOdd")),
            pd_named("isOdd", mk("isEven")),
        ];
        let r = grade(&simple_task(vec![]), &batch, &cdb, 0, 50).unwrap();
        assert!(r.hallucinated_symbols.is_empty());
    }

    #[test]
    fn ungraded_tests_never_silently_pass() {
        let mut task = simple_task(vec![]);
        task.grade.tests = vec!["tests/spec.claw".into()];
        let cdb = Cdb::in_memory().unwrap();
        let produced = Def::new(Expr::Lit(Lit::Int(9)), named("Nat"));
        let r = grade(&task, &[pd(produced)], &cdb, 0, 10).unwrap();
        assert_eq!(r.tests_passed, (0, 1));
        assert!(!r.pass, "tasks with unexecuted oracles must not pass");
    }

    #[test]
    fn scope_seeds_cdb_with_queryable_symbols() {
        let mut task = simple_task(vec![]);
        task.scope = vec![
            ScopeEntry {
                name: "Nat.checkedSub".into(),
                ty: "Nat, Nat -> Result Nat MathErr".into(),
                deprecated: false,
            },
            ScopeEntry {
                name: "Nat.max".into(),
                ty: "Nat, Nat -> Nat".into(),
                deprecated: false,
            },
        ];
        let cdb = task.build_scope_cdb().unwrap();
        assert_eq!(cdb.symbols().unwrap().len(), 2);
        // type-directed lookup works over seeded scope
        let q = claw_core::parse::parse_type("Nat, Nat -> a").unwrap();
        let found = cdb.candidates(&q).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn bad_scope_type_is_a_loud_error() {
        let mut task = simple_task(vec![]);
        task.scope = vec![ScopeEntry {
            name: "broken".into(),
            ty: "Nat,".into(),
            deprecated: false,
        }];
        assert!(task.build_scope_cdb().is_err());
    }

    #[test]
    fn task_schema_roundtrips_from_json() {
        let json = r#"{
            "id": "wallet-transfer-001",
            "category": "repo-feature",
            "prompt": "Implement transfer respecting the Ledger invariant.",
            "grade": {
                "compile": true,
                "tests": ["tests/transfer_spec.claw"],
                "contracts": ["from'.balance == from.balance - amt"],
                "forbidden": ["unsafe", "hallucinated-symbol"]
            },
            "reference": "solutions/wallet-transfer-001.claw"
        }"#;
        let t: Task = serde_json::from_str(json).unwrap();
        assert_eq!(t.id, "wallet-transfer-001");
        assert!(matches!(t.category, Category::RepoFeature));
        assert_eq!(t.grade.contracts.len(), 1);
    }
}
