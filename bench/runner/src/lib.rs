//! claw-bench-runner — drives models through benchmark tasks (WS-J).
//!
//! Arms (docs/benchmark-harness.md §4):
//!   A0 — baseline: prompt only, no CDB context
//!   A1 — +context: in-scope symbol table included in the prompt
//!   A2 — +mask: decode constrained to the scope-mask's GBNF grammar
//!
//! Prototype protocol: the model emits produced definitions as JSON
//! (`Vec<Def>` in claw-core's serde format). Parse failures feed back as
//! retry context, grading is deterministic (claw-bench-grader). The real
//! surface syntax replaces the JSON protocol when the compiler lands; the
//! arms, retry loop, and reporting stay fixed.

use claw_bench_grader::{grade, GradeResult, ProducedDef, Task};
use claw_constraint::{legal_continuations, HoleContext};
use claw_core::Type;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arm {
    A0,
    A1,
    A2,
}

impl std::str::FromStr for Arm {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "A0" => Ok(Arm::A0),
            "A1" => Ok(Arm::A1),
            "A2" => Ok(Arm::A2),
            other => anyhow::bail!("unknown arm `{other}` (expected A0|A1|A2)"),
        }
    }
}

/// Anything that can produce a completion for a prompt.
/// `MockGenerator` for deterministic CI; `HttpGenerator` for real models.
pub trait Generate {
    fn generate(&mut self, prompt: &str) -> anyhow::Result<String>;
    /// Constrain subsequent generations to a GBNF grammar (llama.cpp
    /// style). `None` clears. Default: ignore (endpoints without
    /// grammar support run unconstrained — the report still shows it).
    fn set_grammar(&mut self, _grammar: Option<String>) {}
    /// Rough token accounting for the report (prototype: chars/4).
    fn tokens_used(&self) -> u64;
}

// ---------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------

/// Deterministic generator for tests/CI: pops canned responses in order.
pub struct MockGenerator {
    responses: Vec<String>,
    cursor: usize,
    tokens: u64,
    /// Last grammar set — lets tests assert A2 actually constrained.
    pub last_grammar: Option<String>,
}

impl MockGenerator {
    pub fn new(responses: Vec<String>) -> Self {
        MockGenerator {
            responses,
            cursor: 0,
            tokens: 0,
            last_grammar: None,
        }
    }
}

impl Generate for MockGenerator {
    fn generate(&mut self, prompt: &str) -> anyhow::Result<String> {
        self.tokens += (prompt.len() as u64) / 4;
        let r = self
            .responses
            .get(self.cursor)
            .cloned()
            .unwrap_or_else(|| self.responses.last().cloned().unwrap_or_default());
        self.cursor += 1;
        self.tokens += (r.len() as u64) / 4;
        Ok(r)
    }

    fn set_grammar(&mut self, grammar: Option<String>) {
        self.last_grammar = grammar;
    }

    fn tokens_used(&self) -> u64 {
        self.tokens
    }
}

/// Shell-command generator: spawns a CLI (e.g. `vikasit run --pure -m
/// provider/model`, or any agent CLI) with the prompt appended as the
/// final argument; stdout is the completion. Grammar constraints are
/// ignored (CLI agents don't take logit masks) — do not use for arm A2.
pub struct CmdGenerator {
    argv: Vec<String>,
    tokens: u64,
}

impl CmdGenerator {
    /// From CLAW_MODEL_CMD, e.g.
    /// `CLAW_MODEL_CMD="vikasit run --pure -m opencode/deepseek-v4-flash-free"`.
    pub fn from_env() -> anyhow::Result<Self> {
        let raw = std::env::var("CLAW_MODEL_CMD")
            .map_err(|_| anyhow::anyhow!("CLAW_MODEL_CMD not set"))?;
        let argv: Vec<String> = raw.split_whitespace().map(String::from).collect();
        anyhow::ensure!(!argv.is_empty(), "CLAW_MODEL_CMD is empty");
        Ok(CmdGenerator { argv, tokens: 0 })
    }
}

impl Generate for CmdGenerator {
    fn generate(&mut self, prompt: &str) -> anyhow::Result<String> {
        self.tokens += (prompt.len() as u64) / 4;
        let out = std::process::Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .arg(prompt)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "generator command failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        self.tokens += (text.len() as u64) / 4;
        Ok(text)
    }

    fn tokens_used(&self) -> u64 {
        self.tokens
    }
}

/// OpenAI-compatible chat-completions client.
/// Config via env: CLAW_MODEL_URL (e.g. http://localhost:8000/v1),
/// CLAW_MODEL_NAME, CLAW_MODEL_KEY (optional).
pub struct HttpGenerator {
    base_url: String,
    model: String,
    api_key: Option<String>,
    tokens: u64,
    grammar: Option<String>,
}

impl HttpGenerator {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(HttpGenerator {
            base_url: std::env::var("CLAW_MODEL_URL")
                .map_err(|_| anyhow::anyhow!("CLAW_MODEL_URL not set"))?,
            model: std::env::var("CLAW_MODEL_NAME").unwrap_or_else(|_| "default".into()),
            api_key: std::env::var("CLAW_MODEL_KEY").ok(),
            tokens: 0,
            grammar: None,
        })
    }
}

impl Generate for HttpGenerator {
    fn generate(&mut self, prompt: &str) -> anyhow::Result<String> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut req = ureq::post(&url).set("content-type", "application/json");
        if let Some(k) = &self.api_key {
            req = req.set("authorization", &format!("Bearer {k}"));
        }
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.2,
        });
        // llama.cpp-server-style grammar constraint; OpenAI-compatible
        // servers without support ignore unknown fields.
        if let Some(g) = &self.grammar {
            body["grammar"] = serde_json::Value::String(g.clone());
        }
        let resp: serde_json::Value = req.send_json(body)?.into_json()?;
        if let Some(u) = resp.get("usage").and_then(|u| u.get("total_tokens")) {
            self.tokens += u.as_u64().unwrap_or(0);
        }
        resp["choices"][0]["message"]["content"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("malformed completion response"))
    }

    fn set_grammar(&mut self, grammar: Option<String>) {
        self.grammar = grammar;
    }

    fn tokens_used(&self) -> u64 {
        self.tokens
    }
}

// ---------------------------------------------------------------------
// Prompt building
// ---------------------------------------------------------------------

const OUTPUT_PROTOCOL: &str = r#"
Output ONLY a JSON array of definitions, no prose, no code fences.
Definition schema (serde):
  {"name": "myFn", "expr": <Expr>, "ty": <Type>, "effects": [], "deprecated": false, "doc": ""}
"name" is the definition you are producing; it may reference itself
(recursion) or a sibling definition in this same array.
Expr: {"Var": "name"} | {"Ref": "hash"} | {"Lit": {"Int": 1}} | {"Lit": {"Str": "s"}}
    | {"Lam": {"params": ["p0"], "body": <Expr>}}
    | {"App": {"func": <Expr>, "args": [<Expr>]}}
Type: {"Named": "Nat"} | {"Var": "a"} | {"App": ["Result", [<Type>, <Type>]]}
    | {"Fn": [[<Type>], <Type>]}
Lambda parameters MUST be named p0, p1, p2, … (in order). Reference a
parameter, an in-scope symbol, or your own "name"/siblings with
{"Var": "<name>"}. Do NOT invent any other name."#;

pub fn build_prompt(task: &Task, arm: Arm, prior_feedback: &[String]) -> String {
    let mut p = String::new();
    p.push_str(&format!("Task: {}\n", task.prompt));
    if arm != Arm::A0 && !task.scope.is_empty() {
        p.push_str("\nIn-scope symbols (the ONLY callable definitions):\n");
        for s in &task.scope {
            if !s.deprecated {
                p.push_str(&format!("  {} : {}\n", s.name, s.ty));
            }
        }
    }
    p.push_str(OUTPUT_PROTOCOL);
    for (i, fb) in prior_feedback.iter().enumerate() {
        p.push_str(&format!("\n\nAttempt {} failed: {}", i + 1, fb));
    }
    p
}

/// Parse the model's output into produced definitions. Tolerates code
/// fences. The `name` field is optional so older name-less outputs still
/// parse (they just can't self-reference).
pub fn parse_output(raw: &str) -> anyhow::Result<Vec<ProducedDef>> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    Ok(serde_json::from_str(cleaned)?)
}

// ---------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------

pub struct RunConfig {
    pub arm: Arm,
    pub max_retries: u32,
}

/// Run one task: generate → parse → grade → feed diagnostics back, up to
/// max_retries. Returns the final grade.
pub fn run_task(
    task: &Task,
    cfg: &RunConfig,
    generator: &mut dyn Generate,
) -> anyhow::Result<GradeResult> {
    let cdb = task.build_scope_cdb()?;

    // A2: constrain decoding to the mask's GBNF projection. Scope symbols
    // become an explicit grammar alternation — out-of-scope library calls
    // are ungeneratable at the token level.
    if cfg.arm == Arm::A2 {
        let hole = HoleContext {
            editing: None,
            expected: Type::Var("any".into()),
        };
        let mask = legal_continuations(&cdb, &hole)?;
        generator.set_grammar(Some(mask.to_gbnf()));
    } else {
        generator.set_grammar(None);
    }
    let mut feedback: Vec<String> = Vec::new();
    let mut last: Option<GradeResult> = None;

    for attempt in 0..=cfg.max_retries {
        let prompt = build_prompt(task, cfg.arm, &feedback);
        let raw = generator.generate(&prompt)?;
        match parse_output(&raw) {
            Err(e) => {
                feedback.push(format!("output did not parse as Def JSON: {e}"));
                // record a failed attempt so an unparseable final round grades as failure
                last = Some(GradeResult {
                    task_id: task.id.clone(),
                    compiled: false,
                    tests_passed: (0, task.grade.tests.len() as u32),
                    contracts_held: (0, task.grade.contracts.len() as u32),
                    forbidden_hit: vec![],
                    hallucinated_symbols: vec![],
                    effect_unsound: vec![],
                    pass: false,
                    retries_used: attempt,
                    tokens: generator.tokens_used(),
                });
            }
            Ok(defs) => {
                let result = grade(task, &defs, &cdb, attempt, generator.tokens_used())?;
                let done = result.pass || result.compiled;
                if !done {
                    feedback.push(format!(
                        "hallucinated symbols: [{}]. Use only in-scope symbols.",
                        result.hallucinated_symbols.join(", ")
                    ));
                }
                let finished = result.pass;
                last = Some(result);
                if finished {
                    break;
                }
            }
        }
    }
    last.ok_or_else(|| anyhow::anyhow!("no attempts ran"))
}

// ---------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Report {
    pub arm: Arm,
    pub tasks: usize,
    pub compiled: usize,
    pub passed: usize,
    pub with_hallucinations: usize,
    pub total_hallucinated_symbols: usize,
    pub total_tokens: u64,
    pub results: Vec<GradeResult>,
}

pub fn aggregate(arm: Arm, results: Vec<GradeResult>) -> Report {
    Report {
        arm,
        tasks: results.len(),
        compiled: results.iter().filter(|r| r.compiled).count(),
        passed: results.iter().filter(|r| r.pass).count(),
        with_hallucinations: results
            .iter()
            .filter(|r| !r.hallucinated_symbols.is_empty())
            .count(),
        total_hallucinated_symbols: results.iter().map(|r| r.hallucinated_symbols.len()).sum(),
        total_tokens: results.iter().map(|r| r.tokens).max().unwrap_or(0),
        results,
    }
}

impl Report {
    pub fn render_table(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "arm {:?}: {}/{} compiled, {}/{} passed, {} task(s) hallucinated ({} symbols)\n",
            self.arm,
            self.compiled,
            self.tasks,
            self.passed,
            self.tasks,
            self.with_hallucinations,
            self.total_hallucinated_symbols
        ));
        for r in &self.results {
            s.push_str(&format!(
                "  {:<24} compiled={} pass={} halluc={:?} retries={}\n",
                r.task_id, r.compiled, r.pass, r.hallucinated_symbols, r.retries_used
            ));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_bench_grader::{Category, GradeSpec, ScopeEntry};

    fn task_with_scope() -> Task {
        Task {
            id: "double-001".into(),
            category: Category::FromScratch,
            prompt: "Define double".into(),
            scope: vec![ScopeEntry {
                name: "Nat.add".into(),
                ty: "Nat, Nat -> Nat".into(),
                deprecated: false,
            }],
            params: vec![],
            grade: GradeSpec {
                compile: true,
                requires: vec![],
                tests: vec![],
                contracts: vec![],
                forbidden: vec!["hallucinated-symbol".into()],
            },
            reference: None,
        }
    }

    /// \x -> Nat.add x x — clean solution, references only in-scope name.
    fn clean_solution() -> String {
        serde_json::json!([{
            "expr": {"Lam": {"params": ["x"],
                "body": {"App": {"func": {"Var": "Nat.add"},
                                  "args": [{"Var": "x"}, {"Var": "x"}]}}}},
            "ty": {"Fn": [[{"Named": "Nat"}], {"Named": "Nat"}]},
            "effects": [], "deprecated": false, "doc": ""
        }])
        .to_string()
    }

    /// references `magic_double` — a symbol that does not exist.
    fn hallucinating_solution() -> String {
        serde_json::json!([{
            "expr": {"App": {"func": {"Var": "magic_double"}, "args": [{"Lit": {"Int": 2}}]}},
            "ty": {"Named": "Nat"},
            "effects": [], "deprecated": false, "doc": ""
        }])
        .to_string()
    }

    #[test]
    fn clean_run_compiles_and_passes() {
        let task = task_with_scope();
        let mut generator = MockGenerator::new(vec![clean_solution()]);
        let cfg = RunConfig {
            arm: Arm::A1,
            max_retries: 0,
        };
        let r = run_task(&task, &cfg, &mut generator).unwrap();
        assert!(r.compiled);
        assert!(r.pass);
        assert!(r.hallucinated_symbols.is_empty());
    }

    #[test]
    fn hallucination_fails_then_retry_feedback_fixes_it() {
        let task = task_with_scope();
        // first response hallucinates, second (after feedback) is clean
        let mut generator = MockGenerator::new(vec![hallucinating_solution(), clean_solution()]);
        let cfg = RunConfig {
            arm: Arm::A1,
            max_retries: 1,
        };
        let r = run_task(&task, &cfg, &mut generator).unwrap();
        assert!(r.pass, "retry with diagnostic feedback should succeed");
        assert_eq!(r.retries_used, 1);
    }

    #[test]
    fn unparseable_output_grades_as_failure_not_crash() {
        let task = task_with_scope();
        let mut generator = MockGenerator::new(vec!["not json at all".into()]);
        let cfg = RunConfig {
            arm: Arm::A0,
            max_retries: 0,
        };
        let r = run_task(&task, &cfg, &mut generator).unwrap();
        assert!(!r.pass);
        assert!(!r.compiled);
    }

    #[test]
    fn a0_prompt_hides_scope_a1_shows_it() {
        let task = task_with_scope();
        let p0 = build_prompt(&task, Arm::A0, &[]);
        let p1 = build_prompt(&task, Arm::A1, &[]);
        assert!(!p0.contains("Nat.add"), "A0 must not leak scope context");
        assert!(p1.contains("Nat.add : Nat, Nat -> Nat"));
    }

    #[test]
    fn a2_constrains_decoding_with_scope_grammar() {
        let task = task_with_scope();
        let mut generator = MockGenerator::new(vec![clean_solution()]);
        let cfg = RunConfig {
            arm: Arm::A2,
            max_retries: 0,
        };
        let r = run_task(&task, &cfg, &mut generator).unwrap();
        assert!(r.pass);
        let g = generator
            .last_grammar
            .as_deref()
            .expect("A2 must set a grammar");
        assert!(
            g.contains("Nat.add"),
            "scope symbol must be in the grammar alternation"
        );
        assert!(g.contains("root ::="));
    }

    #[test]
    fn non_a2_arms_clear_the_grammar() {
        let task = task_with_scope();
        let mut generator = MockGenerator::new(vec![clean_solution()]);
        generator.last_grammar = Some("stale".into());
        let cfg = RunConfig {
            arm: Arm::A1,
            max_retries: 0,
        };
        run_task(&task, &cfg, &mut generator).unwrap();
        assert!(generator.last_grammar.is_none());
    }

    #[test]
    fn report_aggregates_the_gate_metrics() {
        let task = task_with_scope();
        let cfg = RunConfig {
            arm: Arm::A1,
            max_retries: 0,
        };

        let mut g1 = MockGenerator::new(vec![clean_solution()]);
        let mut g2 = MockGenerator::new(vec![hallucinating_solution()]);
        let results = vec![
            run_task(&task, &cfg, &mut g1).unwrap(),
            run_task(&task, &cfg, &mut g2).unwrap(),
        ];
        let report = aggregate(Arm::A1, results);
        assert_eq!(report.tasks, 2);
        assert_eq!(report.compiled, 1);
        assert_eq!(report.passed, 1);
        assert_eq!(report.with_hallucinations, 1);
        assert!(report.render_table().contains("double-001"));
    }
}
