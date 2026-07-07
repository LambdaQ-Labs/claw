//! `achuk ai` — the bundled model, wired to the guardrails.
//!
//! One command closes the whole loop the language exists for:
//!
//! ```text
//! achuk ai gen "define double : Nat -> Nat"
//!   → prompt = task + the CDB's real symbols + the output protocol
//!   → generation is CONSTRAINED by the scope's GBNF grammar
//!   → the result is typechecked by the real compiler before you see it
//! ```
//!
//! The inference runtime is a bundled llama.cpp server (`achuk-infer`) and
//! a quantized model (`model/achuk-coder-3b-q8.gguf`), both shipped in the same
//! tarball as the compiler — no separate downloads, no configuration.
//! `achuk ai gen` starts the server on demand and leaves it warm.

use achuk_cdb::Cdb;
use std::path::{Path, PathBuf};

const PORT: u16 = 8873;

/// The exact output protocol the model was fine-tuned on (train/train.py).
const PROTOCOL: &str = r#"Output ONLY a JSON array of definitions, no prose, no code fences.
Definition schema: {"name": str, "expr": <Expr>, "ty": <Type>, "effects": [<str>], "deprecated": false, "doc": ""}
Expr: {"Var": name} | {"Lit": {"Int": n}} | {"Lit": {"Str": s}} | {"Lam": {"params": ["p0"], "body": <Expr>}} | {"App": {"func": <Expr>, "args": [<Expr>]}}
Type: {"Named": "Nat"} | {"Var": "a"} | {"App": ["Result", [<Type>]]} | {"Fn": [[<Type>], <Type>]}
Lambda parameters MUST be named p0, p1, ... Reference a parameter or an in-scope symbol with {"Var": "<name>"}. Do NOT invent any other name.
"effects" lists the effect row: the union of the effect rows of every effectful in-scope symbol the code uses (e.g. ["Fs"] when calling File.read!); [] for pure code.
When defining MULTIPLE definitions, name helpers from the pool: step, helper, aux, go, part — a sibling may then be referenced with {"Var": "<helper name>"}."#;

/// Locate a bundled resource: packaged installs keep `bin/` and `model/`
/// side by side; dev checkouts use env overrides.
fn find_resource(env: &str, rel: &[&str], what: &str) -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var(env) {
        let p = PathBuf::from(p);
        anyhow::ensure!(p.exists(), "{env}={} does not exist", p.display());
        return Ok(p);
    }
    let exe = std::env::current_exe()?;
    let root = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("cannot locate the install root"))?;
    let mut p = root.to_path_buf();
    for r in rel {
        p = p.join(r);
    }
    anyhow::ensure!(
        p.exists(),
        "{what} not found at {} (packaged installs bundle it; in a dev checkout set {env})",
        p.display()
    );
    Ok(p)
}

fn model_path() -> anyhow::Result<PathBuf> {
    find_resource("ACHUK_MODEL_PATH", &["model", "achuk-coder-3b-q8.gguf"], "the bundled model")
}

fn infer_path() -> anyhow::Result<PathBuf> {
    let bin = if cfg!(windows) { "achuk-infer.exe" } else { "achuk-infer" };
    find_resource("ACHUK_INFER_PATH", &["bin", bin], "the inference server")
}

fn server_up() -> bool {
    ureq::get(&format!("http://127.0.0.1:{PORT}/health"))
        .timeout(std::time::Duration::from_millis(700))
        .call()
        .is_ok()
}

/// Start the bundled server detached and wait for /health.
fn ensure_server() -> anyhow::Result<()> {
    if server_up() {
        return Ok(());
    }
    let model = model_path()?;
    let infer = infer_path()?;
    eprintln!("starting the bundled model ({}) …", model.file_name().unwrap_or_default().to_string_lossy());
    std::process::Command::new(&infer)
        .args(["-m"])
        .arg(&model)
        .args(["--port", &PORT.to_string(), "--host", "127.0.0.1", "-c", "4096"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("launching {}: {e}", infer.display()))?;
    for _ in 0..120 {
        if server_up() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    anyhow::bail!("the model server did not become healthy within 60s")
}

fn scope_lines(cdb: &Cdb) -> anyhow::Result<String> {
    let mut out = Vec::new();
    for (n, h) in cdb.symbols()? {
        let d = cdb.get(&h)?;
        let eff = if d.effects.is_empty() {
            String::new()
        } else {
            format!("  [effects: {}]", d.effects.join(", "))
        };
        out.push(format!("  {n} : {}{eff}", d.ty));
    }
    Ok(out.join("\n"))
}

/// Collect every `Var` name referenced in an expression (its free-and-bound
/// identifiers). Used to stub only the scope symbols a candidate actually
/// calls, keeping the verification module minimal.
fn collect_refs(e: &achuk_core::Expr, out: &mut std::collections::HashSet<String>) {
    use achuk_core::Expr::*;
    match e {
        Var(v) => {
            out.insert(v.clone());
        }
        Lam { body, .. } => collect_refs(body, out),
        App { func, args } => {
            collect_refs(func, out);
            args.iter().for_each(|a| collect_refs(a, out));
        }
        If { cond, then, els } => {
            collect_refs(cond, out);
            collect_refs(then, out);
            collect_refs(els, out);
        }
        Let { value, body, .. } => {
            collect_refs(value, out);
            collect_refs(body, out);
        }
        Record(fields) => fields.iter().for_each(|(_, e)| collect_refs(e, out)),
        Field(e, _) => collect_refs(e, out),
        Tag(_, args) => args.iter().for_each(|a| collect_refs(a, out)),
        Match(s, arms) => {
            collect_refs(s, out);
            arms.iter().for_each(|(_, b)| collect_refs(b, out));
        }
        Ref(_) | Lit(_) => {}
    }
}

/// `achuk ai gen "<task>"` — generate, constrained and verified.
fn gen(cdb: &Cdb, task: &str, unconstrained: bool) -> anyhow::Result<()> {
    use achuk_constraint::{legal_continuations, HoleContext};

    ensure_server()?;

    let scope = scope_lines(cdb)?;
    let prompt = format!(
        "Task: {task}\n\nIn-scope symbols (the ONLY callable definitions):\n{scope}\n\n{PROTOCOL}"
    );

    let grammar = if unconstrained {
        None
    } else {
        let hole = HoleContext {
            editing: None,
            expected: achuk_core::Type::Var("any".into()),
        };
        Some(legal_continuations(cdb, &hole)?.to_gbnf())
    };

    // Scope symbols for the real-compiler verification. We stub only the ones
    // a candidate actually references — stubbing the WHOLE cdb pulls in
    // complex-typed symbols (effectful `main!`, polymorphic `where` clauses)
    // whose crash-stubs may not typecheck, false-rejecting correct code.
    let mut all_scope = Vec::new();
    for (n, h) in cdb.symbols()? {
        let d = cdb.get(&h)?;
        all_scope.push((n, d.ty));
    }

    // Best-of-N: the model is small, so one greedy shot often fails. We
    // sample up to N candidates (attempt 0 greedy, rest sampled) and return
    // the FIRST that the real compiler accepts — the compiler is the filter.
    // This is the whole thesis: a weak model made reliable by verification.
    const N: usize = 6;
    let mut last_fail: Option<(Vec<achuk_bench_grader::ProducedDef>, String, achuk_bench_grader::realc::RealCheck)> = None;

    for attempt in 0..N {
        let mut body = serde_json::json!({
            "messages": [{"role": "user", "content": prompt}],
            "temperature": if attempt == 0 { 0.0 } else { 0.7 },
            "seed": attempt as i64,
            "max_tokens": 512,
        });
        if let Some(g) = &grammar {
            body["grammar"] = serde_json::Value::String(g.clone());
        }
        if attempt == 1 {
            eprint!("  refining");
        } else if attempt > 1 {
            eprint!(".");
        }

        let resp: serde_json::Value =
            ureq::post(&format!("http://127.0.0.1:{PORT}/v1/chat/completions"))
                .set("content-type", "application/json")
                .timeout(std::time::Duration::from_secs(600))
                .send_string(&body.to_string())?
                .into_json()?;
        let raw = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .trim_matches('`')
            .to_string();

        let defs: Vec<achuk_bench_grader::ProducedDef> = match serde_json::from_str(&raw) {
            Ok(d) => d,
            Err(_) => continue, // unparseable — try another sample
        };
        // Keep only the scope symbols this candidate references.
        let mut refs = std::collections::HashSet::new();
        for d in &defs {
            collect_refs(&d.def.expr, &mut refs);
        }
        let scope_pairs: Vec<_> = all_scope
            .iter()
            .filter(|(n, _)| refs.contains(n.as_str()))
            .cloned()
            .collect();
        let module = achuk_bench_grader::realc::to_module(&scope_pairs, &defs);
        match achuk_bench_grader::realc::achukc_check(&module) {
            Ok(r) if r.compiled => {
                if attempt > 0 {
                    eprintln!();
                }
                println!("── generated ──");
                for (i, d) in defs.iter().enumerate() {
                    let name = d.name.clone().unwrap_or_else(|| format!("def{i}"));
                    println!("{}", achuk_core::render::render_def(&name, &d.def));
                }
                let note = if attempt == 0 {
                    String::new()
                } else {
                    format!("  (verified on attempt {} of {N})", attempt + 1)
                };
                println!("── verified ── real compiler: OK{note}");
                achuk_telemetry::event(
                    "ai_gen",
                    serde_json::json!({"constrained": !unconstrained, "defs": defs.len(), "attempts": attempt + 1, "ok": true}),
                    Some(serde_json::json!({"task": task, "raw": raw})),
                );
                return Ok(());
            }
            Ok(r) => last_fail = Some((defs, raw, r)),
            Err(e) => {
                println!("── unverified ── compiler unavailable ({e})");
                return Ok(());
            }
        }
    }

    // Every candidate failed the compiler — show the closest attempt.
    if let Some((defs, raw, r)) = last_fail {
        eprintln!();
        println!("── generated ── (best of {N} attempts)");
        for (i, d) in defs.iter().enumerate() {
            let name = d.name.clone().unwrap_or_else(|| format!("def{i}"));
            println!("{}", achuk_core::render::render_def(&name, &d.def));
        }
        println!("── REJECTED ── none of {N} attempts compiled ({} error(s)):\n{}", r.errors, r.detail);
        println!(
            "\nThe bundled model is a small 0.5B — it struggles with some tasks. \
             Try rephrasing the task, breaking it into smaller pieces, or `achuk index .` \
             so the AI sees more of your real code."
        );
        achuk_telemetry::event(
            "ai_gen",
            serde_json::json!({"constrained": !unconstrained, "attempts": N, "ok": false}),
            Some(serde_json::json!({"task": task, "raw": raw})),
        );
        std::process::exit(1);
    }
    anyhow::bail!("the model produced no parseable output in {N} attempts")
}


pub fn ai_cmd(db_path: &Path, args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("status") => {
            match model_path() {
                Ok(p) => println!("model:  {} ({} MB)", p.display(),
                    std::fs::metadata(&p).map(|m| m.len() / 1_048_576).unwrap_or(0)),
                Err(e) => println!("model:  missing — {e}"),
            }
            match infer_path() {
                Ok(p) => println!("server: {}", p.display()),
                Err(e) => println!("server: missing — {e}"),
            }
            println!("state:  {}", if server_up() { format!("running on :{PORT}") } else { "not running (starts on first `achuk ai gen`)".into() });
            Ok(())
        }
        Some("serve") => {
            ensure_server()?;
            println!("model server running on http://127.0.0.1:{PORT}");
            Ok(())
        }
        Some("stop") => {
            // Best-effort: the server binds our fixed port; find and stop it.
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("pkill")
                    .args(["-f", "achuk-infer"])
                    .status();
            }
            println!("stopped (if it was running)");
            Ok(())
        }
        Some("gen") => {
            let task = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("usage: achuk ai gen \"<what to define>\" [--unconstrained]")
            })?;
            let cdb = Cdb::open(db_path)?;
            gen(&cdb, task, args.iter().any(|a| a == "--unconstrained"))
        }
        _ => {
            println!("achuk ai — the bundled model, wired to the guardrails\n");
            println!("  achuk ai gen \"<task>\"   generate a definition (grammar-constrained, compiler-verified)");
            println!("  achuk ai serve           start the model server (gen does this automatically)");
            println!("  achuk ai status          where the model and server are");
            println!("  achuk ai stop            stop the model server");
            Ok(())
        }
    }
}
