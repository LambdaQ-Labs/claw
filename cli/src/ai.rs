//! `claw ai` — the bundled model, wired to the guardrails.
//!
//! One command closes the whole loop the language exists for:
//!
//! ```text
//! claw ai gen "define double : Nat -> Nat"
//!   → prompt = task + the CDB's real symbols + the output protocol
//!   → generation is CONSTRAINED by the scope's GBNF grammar
//!   → the result is typechecked by the real compiler before you see it
//! ```
//!
//! The inference runtime is a bundled llama.cpp server (`claw-infer`) and
//! a quantized model (`model/claw-0.5b-q8.gguf`), both shipped in the same
//! tarball as the compiler — no separate downloads, no configuration.
//! `claw ai gen` starts the server on demand and leaves it warm.

use claw_cdb::Cdb;
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
    find_resource("CLAW_MODEL_PATH", &["model", "claw-0.5b-q8.gguf"], "the bundled model")
}

fn infer_path() -> anyhow::Result<PathBuf> {
    let bin = if cfg!(windows) { "claw-infer.exe" } else { "claw-infer" };
    find_resource("CLAW_INFER_PATH", &["bin", bin], "the inference server")
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

/// `claw ai gen "<task>"` — generate, constrained and verified.
fn gen(cdb: &Cdb, task: &str, unconstrained: bool) -> anyhow::Result<()> {
    use claw_constraint::{legal_continuations, HoleContext};

    ensure_server()?;

    let scope = scope_lines(cdb)?;
    let prompt = format!(
        "Task: {task}\n\nIn-scope symbols (the ONLY callable definitions):\n{scope}\n\n{PROTOCOL}"
    );

    let mut body = serde_json::json!({
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0,
        "max_tokens": 300,
    });
    if !unconstrained {
        let hole = HoleContext {
            editing: None,
            expected: claw_core::Type::Var("any".into()),
        };
        let mask = legal_continuations(cdb, &hole)?;
        body["grammar"] = serde_json::Value::String(mask.to_gbnf());
    }

    let resp: serde_json::Value = ureq::post(&format!("http://127.0.0.1:{PORT}/v1/chat/completions"))
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(600))
        .send_string(&body.to_string())?
        .into_json()?;
    let raw = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no completion in the server response"))?;

    let defs: Vec<claw_bench_grader::ProducedDef> =
        serde_json::from_str(raw.trim().trim_matches('`'))
            .map_err(|e| anyhow::anyhow!("model output was not Def-JSON: {e}\n{raw}"))?;

    // Render for the human.
    println!("── generated ──");
    for (i, d) in defs.iter().enumerate() {
        let name = d.name.clone().unwrap_or_else(|| format!("def{i}"));
        println!("{}", claw_core::render::render_def(&name, &d.def));
    }

    // Verify with the real compiler: every CDB symbol in scope as a stub.
    let mut scope_pairs = Vec::new();
    for (n, h) in cdb.symbols()? {
        let d = cdb.get(&h)?;
        scope_pairs.push((n, d.ty));
    }
    let module = claw_bench_grader::realc::to_module(&scope_pairs, &defs);
    match claw_bench_grader::realc::clawc_check(&module) {
        Ok(r) if r.compiled => println!("── verified ── real compiler: OK"),
        Ok(r) => {
            println!("── REJECTED ── real compiler found {} error(s):\n{}", r.errors, r.detail);
            std::process::exit(1);
        }
        Err(e) => println!("── unverified ── compiler unavailable ({e})"),
    }

    claw_telemetry::event(
        "ai_gen",
        serde_json::json!({"constrained": !unconstrained, "defs": defs.len()}),
        Some(serde_json::json!({"task": task, "raw": raw})),
    );
    Ok(())
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
            println!("state:  {}", if server_up() { format!("running on :{PORT}") } else { "not running (starts on first `claw ai gen`)".into() });
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
                    .args(["-f", "claw-infer"])
                    .status();
            }
            println!("stopped (if it was running)");
            Ok(())
        }
        Some("gen") => {
            let task = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("usage: claw ai gen \"<what to define>\" [--unconstrained]")
            })?;
            let cdb = Cdb::open(db_path)?;
            gen(&cdb, task, args.iter().any(|a| a == "--unconstrained"))
        }
        _ => {
            println!("claw ai — the bundled model, wired to the guardrails\n");
            println!("  claw ai gen \"<task>\"   generate a definition (grammar-constrained, compiler-verified)");
            println!("  claw ai serve           start the model server (gen does this automatically)");
            println!("  claw ai status          where the model and server are");
            println!("  claw ai stop            stop the model server");
            Ok(())
        }
    }
}
