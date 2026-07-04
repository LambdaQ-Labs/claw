//! claw-bench — benchmark runner CLI (WS-J).
//!
//! Usage:
//!   claw-bench run --arm A0 --tasks bench/tasks [--retries 3] [--json out.json]
//!
//! Model via env — either:
//!   CLAW_MODEL_URL, CLAW_MODEL_NAME, CLAW_MODEL_KEY   (OpenAI-compatible HTTP)
//!   CLAW_MODEL_CMD                                    (CLI generator, e.g.
//!       "vikasit run --pure -m opencode/deepseek-v4-flash-free")
//! CLAW_MODEL_CMD wins if both are set. CLI generators can't take logit
//! masks — arm A2 refuses to run with one.

use claw_bench_grader::Task;
use claw_bench_runner::{
    aggregate, run_task, Arm, CmdGenerator, Generate, HttpGenerator, RunConfig,
};
use std::path::PathBuf;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("run") {
        anyhow::bail!(
            "usage: claw-bench run --arm A0|A1 --tasks <dir> [--retries N] [--json <out>]"
        );
    }

    let mut arm = Arm::A0;
    let mut tasks_dir = PathBuf::from("bench/tasks");
    let mut retries: u32 = 3;
    let mut json_out: Option<PathBuf> = None;
    let mut distill_out: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--arm" => {
                arm = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--arm needs a value"))?
                    .parse()?;
                i += 2;
            }
            "--tasks" => {
                tasks_dir = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--tasks needs a value"))?
                    .into();
                i += 2;
            }
            "--retries" => {
                retries = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--retries needs a value"))?
                    .parse()?;
                i += 2;
            }
            "--json" => {
                json_out = Some(
                    args.get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--json needs a value"))?
                        .into(),
                );
                i += 2;
            }
            "--distill" => {
                distill_out = Some(
                    args.get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--distill needs a value"))?
                        .into(),
                );
                i += 2;
            }
            other => anyhow::bail!("unknown flag `{other}`"),
        }
    }

    // Load tasks
    let mut tasks: Vec<Task> = Vec::new();
    for entry in std::fs::read_dir(&tasks_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let raw = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<Task>(&raw) {
                Ok(t) => tasks.push(t),
                Err(e) => eprintln!("skipping {}: {e}", path.display()), // loud, not silent
            }
        }
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    anyhow::ensure!(
        !tasks.is_empty(),
        "no tasks found in {}",
        tasks_dir.display()
    );
    eprintln!(
        "running {} task(s), arm {:?}, retries {}",
        tasks.len(),
        arm,
        retries
    );

    let use_cmd = std::env::var("CLAW_MODEL_CMD").is_ok();

    // Distillation: generate with a strong model, keep only grader-VERIFIED
    // completions (compiled ∧ no hallucination), write as SFT corpus. This
    // is how HuggingFace-sourced (or procedural) prompts become training
    // data — the completions are Claw, machine-checked, never hallucinated.
    if let Some(out) = &distill_out {
        return distill(&tasks, out, use_cmd);
    }

    let cfg = RunConfig {
        arm,
        max_retries: retries,
    };
    let mut results = Vec::new();
    let mut errored: Vec<(String, String)> = Vec::new();

    if use_cmd && arm == Arm::A2 {
        anyhow::bail!(
            "arm A2 needs a grammar-honoring HTTP endpoint; CLI generators can't take logit masks"
        );
    }

    for task in &tasks {
        // fresh generator per task: no cross-task context bleed
        let mut generator: Box<dyn Generate> = if use_cmd {
            Box::new(CmdGenerator::from_env()?)
        } else {
            Box::new(HttpGenerator::from_env()?)
        };
        match run_task(task, &cfg, &mut *generator) {
            Ok(r) => {
                eprintln!("  {} compiled={} pass={}", task.id, r.compiled, r.pass);
                results.push(r);
            }
            Err(e) => {
                eprintln!("  {} ERROR: {e}", task.id);
                errored.push((task.id.clone(), e.to_string()));
            }
        }
    }

    let report = aggregate(arm, results);
    println!("{}", report.render_table());
    if !errored.is_empty() {
        // no silent truncation: errored tasks are reported, not dropped
        println!("{} task(s) errored (not graded):", errored.len());
        for (id, e) in &errored {
            println!("  {id}: {e}");
        }
    }
    if let Some(path) = json_out {
        std::fs::write(&path, serde_json::to_string_pretty(&report)?)?;
        eprintln!("report written to {}", path.display());
    }
    Ok(())
}

/// Distill a verified SFT corpus: for each task, generate with the model,
/// grade, and keep the (prompt, completion) pair only if it compiled with
/// no hallucinated symbols. Output is JSONL matching claw-corpus::Example.
fn distill(tasks: &[Task], out: &std::path::Path, use_cmd: bool) -> anyhow::Result<()> {
    use claw_bench_runner::{build_prompt, grade_produced, parse_output, Arm};
    use std::io::Write;

    let mut file = std::fs::File::create(out)?;
    let (mut kept, mut tried) = (0usize, 0usize);

    for task in tasks {
        tried += 1;
        let mut generator: Box<dyn Generate> = if use_cmd {
            Box::new(CmdGenerator::from_env()?)
        } else {
            Box::new(HttpGenerator::from_env()?)
        };
        // A1-style prompt (scope shown), single shot.
        let prompt = build_prompt(task, Arm::A1, &[]);
        let raw = match generator.generate(&prompt) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  {} gen error: {e}", task.id);
                continue;
            }
        };
        let defs = match parse_output(&raw) {
            Ok(d) => d,
            Err(_) => continue, // unparseable → not verified
        };
        // Verify with the grader (compiled ∧ no hallucination).
        match grade_produced(task, &defs) {
            Ok(r) if r.compiled && r.hallucinated_symbols.is_empty() => {
                let normalized = serde_json::to_string(&defs)?;
                let ex = serde_json::json!({
                    "prompt": task.prompt,
                    "completion": normalized,
                    "uses": [],
                });
                writeln!(file, "{ex}")?;
                kept += 1;
                eprintln!("  {} ✓ kept", task.id);
            }
            _ => eprintln!("  {} ✗ not verified", task.id),
        }
    }
    eprintln!(
        "distilled {kept}/{tried} verified examples → {}",
        out.display()
    );
    Ok(())
}
