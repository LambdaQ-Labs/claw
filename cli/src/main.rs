//! achuk — the Achuk toolchain CLI (WS-I).
//!
//! MVP surface: the code-as-database commands (docs/p2-spec.md §1.6).
//! The compiler subcommands (`achuk build`, `achuk check`) attach here once
//! the vendored compiler is wired up.
//!
//!   achuk db symbols                          list bound names
//!   achuk db put < def.json                   insert a definition (stdin)
//!   achuk db bind <name> <hash>               point a name at a hash
//!   achuk db resolve <name>                   name -> hash
//!   achuk db candidates "<type sig>"          type-directed symbol query
//!   achuk db callers <name|hash>              who references this
//!   achuk db deps <name|hash>                 what this references
//!   achuk db render <name|hash>               definition as JSON
//!   achuk db mask "<type sig>"                legal continuations + GBNF
//!
//! Store path: --db <file> (default ./achuk.cdb).

use achuk_cdb::Cdb;
use achuk_constraint::{legal_continuations, HoleContext, Mask};
use achuk_core::{parse::parse_type, Def, Hash};
use std::io::Read;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

mod ai;

fn real_main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // extract --db <path> anywhere in the argv
    let mut db_path = PathBuf::from("achuk.cdb");
    if let Some(i) = args.iter().position(|a| a == "--db") {
        anyhow::ensure!(i + 1 < args.len(), "--db needs a value");
        db_path = PathBuf::from(args.remove(i + 1));
        args.remove(i);
    }

    // Record the command invoked (name only — never args, paths, or code)
    // so usage is captured for every command, not just `ai`/`defs-check`.
    // Fire before dispatch: some commands exec the compiler and never return.
    {
        let cmd = args.first().map(String::as_str).unwrap_or("help");
        let sub = match cmd {
            "db" | "mcp" | "corpus" | "ai" | "telemetry" => {
                args.get(1).map(|s| format!("{cmd}.{s}")).unwrap_or_else(|| cmd.into())
            }
            _ => cmd.into(),
        };
        achuk_telemetry::event("command", serde_json::json!({ "cmd": sub }), None);
    }

    match args.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            println!("achuk {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("db") => db_cmd(&db_path, &args[1..]),
        // Project model.
        Some("new") => new_cmd(&args[1..]),
        Some("run") => run_cmd(&args[1..]),
        // Compiler passthrough: `achuk check|build|fmt|test|repl <args>` runs
        // the vendored compiler (achukc). ACHUK_COMPILER overrides discovery.
        Some(cmd @ ("check" | "build" | "fmt" | "test" | "repl")) => {
            let status = std::process::Command::new(find_achukc()?)
                .arg(cmd)
                .args(&args[1..])
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        // WS-G: transpile a Def-JSON file (the benchmark protocol) to Rust.
        Some("emit-rust") => emit_rust_cmd(&args[1..]),
        // WS-J: real-compiler compile signal — render Def-JSON + task scope
        // as a .achuk module and run `achukc check` on it. `--batch` grades an
        // outputs.jsonl ({"task": <file>, "defs": [...]} per line).
        Some("defs-check") => defs_check_cmd(&args[1..]),
        // A2 support: print the GBNF grammar for a task's scope (the same
        // projection the bench runner uses to constrain decoding).
        Some("task-grammar") => task_grammar_cmd(&args[1..]),
        // Anonymous usage metrics (on by default; `achuk telemetry off`).
        Some("telemetry") => telemetry_cmd(&args[1..]),
        // Full grade (compile proxy + contract EXECUTION) as JSON — the
        // Achuk side of the cross-language parity harness.
        Some("defs-grade") => defs_grade_cmd(&args[1..]),
        // Self-update from GitHub Releases (packaged installs only).
        Some("upgrade") => upgrade_cmd(&args[1..]),
        // The bundled model: generate -> grammar-constrain -> compiler-verify.
        Some("ai") => ai::ai_cmd(&db_path, &args[1..]),
        // WS-H: generate a synthetic SFT corpus (JSONL). `--stdlib` uses the
        // built-in stdlib scope; otherwise reads the CDB at --db.
        Some("corpus") if args.get(1).map(String::as_str) == Some("gen") => {
            corpus_gen_cmd(&db_path, args.iter().any(|a| a == "--stdlib"))
        }
        // Index a whole project's .achuk files into the CDB so the AI
        // guardrail (candidates/mask/MCP) answers over the user's real code.
        Some("index") => index_cmd(&db_path, &args[1..]),
        // Register the MCP server with an agent (Claude Code) so it writes
        // Achuk grounded in the project's real symbols.
        Some("mcp") if args.get(1).map(String::as_str) == Some("install") => mcp_install_cmd(),
        // Package manager: publish this package to the registry, or add a
        // dependency from the registry to this project.
        Some("login") => login_cmd(&args[1..]),
        Some("publish") => publish_cmd(&args[1..]),
        Some("add") => add_cmd(&args[1..]),
        _ => {
            eprintln!(
                "achuk — the Achuk toolchain\n\nusage:\n  achuk new <name>                              scaffold a new project\n  achuk run [file.achuk]                         run a program (default: main.achuk)\n  achuk build|check|fmt|test|repl <file.achuk>   compiler passthrough\n  achuk [--db <file>] db <subcommand>           code-as-database\n  achuk ai gen \"<task>\"                          bundled model: generate → verify\n  achuk add <pkg> | achuk publish                 packages (registry.achuk.dev)\n  achuk index <dir>                              ingest sources into the CDB\n  achuk defs-check|defs-grade <defs> <task>      verify AI-generated code\n  achuk task-grammar <task.json>                 decode grammar for a scope\n  achuk mcp install                              wire this project into MCP clients\n  achuk telemetry [off|on|full|share|clear]      usage-metrics controls\n  achuk upgrade [--check]                        self-update\n  achuk emit-rust <defs.json>                    transpile Def-JSON → Rust\n  achuk [--db <file>] corpus gen [--stdlib]      synthetic SFT corpus → JSONL\n\ndb subcommands:\n  symbols | put | bind <name> <hash> | resolve <name> | ingest <file.achuk>\n  candidates \"<type>\" | callers <ref> | deps <ref> | render <ref> | mask \"<type>\""
            );
            std::process::exit(2);
        }
    }
}

/// `achuk new <name> [--platform http|cli]` — scaffold a runnable project.
/// Without --platform, a headerless print-and-compute program. With one, a
/// project targeting a bundled platform (an HTTP server, or stdin/stdout).
fn new_cmd(args: &[String]) -> anyhow::Result<()> {
    // --platform <name>
    let platform = args
        .iter()
        .position(|a| a == "--platform")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let name = args
        .iter()
        .find(|a| !a.starts_with("--") && a.as_str() != platform.as_deref().unwrap_or(""))
        .ok_or_else(|| anyhow::anyhow!("usage: achuk new <name> [--platform http|cli]"))?;
    let dir = Path::new(name);
    anyhow::ensure!(!dir.exists(), "`{name}` already exists");
    std::fs::create_dir_all(dir)?;

    let (entry, source) = match platform.as_deref() {
        None => ("main.achuk", DEFAULT_STARTER.to_string()),
        Some(p) => {
            // Copy the bundled platform into the project, generate an app.
            let src = find_platform(p)?;
            copy_dir(&src, &dir.join("platform"))?;
            (
                "app.achuk",
                match p {
                    "http" => HTTP_STARTER.to_string(),
                    "cli" => CLI_STARTER.to_string(),
                    other => anyhow::bail!("unknown platform `{other}` (try: http, cli)"),
                },
            )
        }
    };

    std::fs::write(dir.join(entry), source)?;
    std::fs::write(
        dir.join("achuk.toml"),
        format!(
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"{entry}\"\nplatform = \"{}\"\n",
            platform.as_deref().unwrap_or("print")
        ),
    )?;
    std::fs::write(dir.join(".gitignore"), "/achuk.cdb\n/dist\n*.o\n")?;
    std::fs::write(
        dir.join("README.md"),
        format!("# {name}\n\nA Achuk project.\n\n```sh\nachuk run\n```\n"),
    )?;

    // Best-effort initial index so the AI guardrail works immediately.
    if let Ok(mut cdb) = Cdb::open(&dir.join("achuk.cdb")) {
        let _ = ingest(&mut cdb, &dir.join(entry));
    }

    eprintln!("created project `{name}`");
    eprintln!("  cd {name} && achuk run");
    Ok(())
}

const DEFAULT_STARTER: &str = "# Welcome to Achuk. Run with `achuk run`.\n\
    greet = |who| \"Hello, ${who}!\"\n\n\
    main! = |_args| {\n    \
    echo!(greet(\"world\"))\n    \
    Ok({})\n\
    }\n";

const HTTP_STARTER: &str = "app [main!] { pf: platform \"./platform/main.roc\" }\n\n\
    # An HTTP handler. The host passes the raw request headers; return a U64.\n\
    # Run `achuk run` — it prints the port it bound, then serves a request.\n\
    main! : Str => U64\n\
    main! = |headers| {\n    \
    if Str.contains(headers, \"X-Auth-Token: let-me-in\") 200\n    \
    else if Str.contains(headers, \"X-Auth-Token:\") 403\n    \
    else 401\n\
    }\n";

const CLI_STARTER: &str = "app [main!] { pf: platform \"./platform/main.roc\" }\n\n\
    import pf.Stdout\n\n\
    main! : List(Str) => Try({}, [Exit(I32), ..])\n\
    main! = |_args| {\n    \
    Stdout.line!(\"Hello from a Achuk CLI app!\")\n    \
    Ok({})\n\
    }\n";

/// The project's entry file: an explicit arg, else `achuk.toml`'s entry,
/// else `main.achuk`. Searches up from the cwd for `achuk.toml`.
fn entry_file(args: &[String]) -> PathBuf {
    if let Some(f) = args.first() {
        return PathBuf::from(f);
    }
    // walk up for achuk.toml → use its dir + entry
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            let toml = dir.join("achuk.toml");
            if toml.exists() {
                let entry = std::fs::read_to_string(&toml)
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .find_map(|l| l.trim().strip_prefix("entry ="))
                            .map(|v| v.trim().trim_matches('"').to_string())
                    })
                    .unwrap_or_else(|| "main.achuk".into());
                return dir.join(entry);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    PathBuf::from("main.achuk")
}

/// `achuk run [file]` — run a program via the compiler (default: main.achuk).
fn run_cmd(args: &[String]) -> anyhow::Result<()> {
    let file = entry_file(args);
    anyhow::ensure!(file.exists(), "no such file: {}", file.display());
    let status = std::process::Command::new(find_achukc()?)
        .arg(&file)
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `achuk emit-rust <defs.json>` — read a JSON array of named definitions
/// (the benchmark's Def-JSON protocol) and print a Rust module.
fn emit_rust_cmd(args: &[String]) -> anyhow::Result<()> {
    use achuk_emit_rust::{emit_fn, NameMap};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct NamedDef {
        #[serde(default)]
        name: Option<String>,
        #[serde(flatten)]
        def: Def,
    }

    let path = need(args, 0, "path to defs.json")?;
    let raw = std::fs::read_to_string(path)?;
    let defs: Vec<NamedDef> = serde_json::from_str(&raw)?;

    // Populate the name map so intra-file references (Expr::Ref by content
    // hash) resolve to each def's Rust identifier.
    let mut names = NameMap::new();
    for (i, d) in defs.iter().enumerate() {
        let n = d.name.clone().unwrap_or_else(|| format!("def{i}"));
        names.insert(d.def.hash().0, n.replace('.', "_"));
    }

    println!("// generated by `achuk emit-rust` — do not edit");
    for (i, d) in defs.iter().enumerate() {
        let name = d.name.clone().unwrap_or_else(|| format!("def{i}"));
        match emit_fn(&name, &d.def, &names) {
            Ok(rust) => println!("\n{rust}"),
            Err(e) => eprintln!("// skipped {name}: {e}"),
        }
    }
    Ok(())
}

/// `achuk defs-check <defs.json> <task.json>` (or `--batch <outputs.jsonl>`)
/// — the REAL compile signal: render the task's scope as signature-true
/// crash-stubs plus the produced defs, and run `achukc check` on the module.
fn defs_check_cmd(args: &[String]) -> anyhow::Result<()> {
    use achuk_bench_grader::{realc, ProducedDef, Task};
    if std::env::var("ACHUK_COMPILER").is_err() {
        std::env::set_var("ACHUK_COMPILER", find_achukc()?);
    }

    let check_one = |task: &Task, defs: &[ProducedDef]| -> anyhow::Result<realc::RealCheck> {
        let module = realc::task_module(&task.scope, defs)?;
        realc::achukc_check(&module)
    };

    if args.first().map(String::as_str) == Some("--batch") {
        let batch = need(args, 1, "path to outputs.jsonl")?;
        #[derive(serde::Deserialize)]
        struct Line {
            task: String,
            defs: serde_json::Value,
        }
        let (mut ok, mut fail, mut skip) = (0u32, 0u32, 0u32);
        for line in std::fs::read_to_string(batch)?.lines().filter(|l| !l.trim().is_empty()) {
            let l: Line = serde_json::from_str(line)?;
            let task: Task = serde_json::from_str(&std::fs::read_to_string(&l.task)?)?;
            let defs: Vec<ProducedDef> = match serde_json::from_value(l.defs) {
                Ok(d) => d,
                Err(_) => {
                    skip += 1;
                    println!("SKIP {} (defs not parseable)", l.task);
                    continue;
                }
            };
            let r = check_one(&task, &defs)?;
            if r.compiled {
                ok += 1;
            } else {
                fail += 1;
                println!("FAIL {} ({} errors)", l.task, r.errors);
            }
        }
        let total = ok + fail + skip;
        println!("real-compile: {ok}/{total} ok, {fail} failed, {skip} unparseable");
        return Ok(());
    }

    let defs_path = need(args, 0, "path to defs.json")?;
    let task_path = need(args, 1, "path to task.json")?;
    let defs: Vec<ProducedDef> = serde_json::from_str(&std::fs::read_to_string(defs_path)?)?;
    let task: Task = serde_json::from_str(&std::fs::read_to_string(task_path)?)?;
    let r = check_one(&task, &defs)?;
    achuk_telemetry::event(
        "defs_check",
        serde_json::json!({"compiled": r.compiled, "errors": r.errors, "task": task.id}),
        Some(serde_json::json!({"prompt": task.prompt, "defs": defs})),
    );
    if r.compiled {
        println!("COMPILE-OK");
    } else {
        println!("COMPILE-FAIL ({} errors)\n{}", r.errors, r.detail);
    }
    Ok(())
}

/// `achuk upgrade [--check]` — self-update from GitHub Releases.
///
/// Flow: resolve the latest tag via the GitHub API, compare with this
/// binary's version, download `achuk-<tag>-<os>-<arch>.tar.gz`, verify the
/// `.sha256` sidecar when the release ships one, unpack next to the
/// current install and swap binaries in place (unix rename semantics).
/// Refuses to run from a dev checkout (target/…) — use git + cargo there.
fn upgrade_cmd(args: &[String]) -> anyhow::Result<()> {
    const REPO: &str = "LambdaQ-Labs/achuk";
    let current = env!("CARGO_PKG_VERSION");

    let latest: serde_json::Value = match ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .set("user-agent", "achuk-upgrade")
    .call()
    {
        Ok(r) => r.into_json()?,
        Err(ureq::Error::Status(404, _)) => {
            println!("installed: {current}\nno releases published yet — nothing to upgrade to");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let tag = latest["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no releases published yet"))?
        .to_string();
    let latest_v = tag.trim_start_matches('v');

    let newer = {
        let parse = |s: &str| -> Vec<u64> {
            s.split('.').map(|p| p.parse().unwrap_or(0)).collect()
        };
        parse(latest_v) > parse(current)
    };
    println!("installed: {current}
latest:    {latest_v}");
    if !newer {
        println!("already up to date");
        return Ok(());
    }
    if args.iter().any(|a| a == "--check") {
        println!("run `achuk upgrade` to install {latest_v}");
        return Ok(());
    }

    // Only self-update a packaged install (…/bin/achuk), never a dev build.
    let exe = std::env::current_exe()?;
    let bin_dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot locate the install dir"))?
        .to_path_buf();
    anyhow::ensure!(
        bin_dir.file_name().map(|n| n == "bin").unwrap_or(false),
        "not a packaged install ({}) — update with git + cargo instead",
        exe.display()
    );

    let (os, arch) = (
        match std::env::consts::OS {
            "macos" => "macos",
            "linux" => "linux",
            other => anyhow::bail!("no prebuilt upgrade for {other}"),
        },
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => anyhow::bail!("no prebuilt upgrade for {other}"),
        },
    );
    let asset = format!("achuk-{tag}-{os}-{arch}.tar.gz");
    let url =
        format!("https://github.com/{REPO}/releases/download/{tag}/{asset}");
    eprintln!("downloading {asset} …");
    let mut buf = Vec::new();
    ureq::get(&url)
        .set("user-agent", "achuk-upgrade")
        .call()?
        .into_reader()
        .read_to_end(&mut buf)?;

    // Integrity: the CI publishes a .sha256 sidecar; verify when present.
    match ureq::get(&format!("{url}.sha256"))
        .set("user-agent", "achuk-upgrade")
        .call()
    {
        Ok(resp) => {
            use sha2::Digest;
            let want = resp.into_string()?.split_whitespace().next().unwrap_or("").to_lowercase();
            let got = format!("{:x}", sha2::Sha256::digest(&buf));
            anyhow::ensure!(want == got, "checksum mismatch — aborting upgrade");
            eprintln!("checksum ok");
        }
        Err(_) => eprintln!("(no checksum published for this asset — skipping verification)"),
    }

    // Unpack to a staging dir, then rename each binary over the old one —
    // on unix a running executable can be replaced this way.
    let stage = bin_dir.parent().unwrap().join(format!(".upgrade-{tag}"));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let tarball = stage.join(&asset);
    std::fs::write(&tarball, &buf)?;
    let ok = std::process::Command::new("tar")
        .args(["-xzf"])
        .arg(&tarball)
        .arg("-C")
        .arg(&stage)
        .status()?
        .success();
    anyhow::ensure!(ok, "unpacking failed");
    let mut swapped = 0;
    for entry in std::fs::read_dir(stage.join("bin"))? {
        let entry = entry?;
        let dest = bin_dir.join(entry.file_name());
        std::fs::rename(entry.path(), &dest)?;
        swapped += 1;
    }
    let _ = std::fs::remove_dir_all(&stage);
    println!("upgraded to {latest_v} ({swapped} binaries swapped)");
    Ok(())
}

/// `achuk defs-grade <defs.json> <task.json>` — grade produced defs against
/// a task (hallucination check + executed contracts) and print the
/// GradeResult as JSON. The parity harness consumes this.
fn defs_grade_cmd(args: &[String]) -> anyhow::Result<()> {
    use achuk_bench_grader::{grade, ProducedDef, Task};
    let defs: Vec<ProducedDef> =
        serde_json::from_str(&std::fs::read_to_string(need(args, 0, "defs.json")?)?)?;
    let task: Task = serde_json::from_str(&std::fs::read_to_string(need(args, 1, "task.json")?)?)?;
    let cdb = task.build_scope_cdb()?;
    let r = grade(&task, &defs, &cdb, 0, 0)?;
    println!("{}", serde_json::to_string(&r)?);
    Ok(())
}

/// `achuk telemetry status|share|clear` — the opt-in usage log. Collection
/// is off unless ACHUK_TELEMETRY=metrics|full; `share` uploads gzipped
/// JSONL to ACHUK_TELEMETRY_URL and clears the local log on success.
fn telemetry_cmd(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("share") => match achuk_telemetry::share() {
            Ok(msg) => println!("{msg}"),
            Err(e) => anyhow::bail!(e),
        },
        Some("clear") => println!("{}", achuk_telemetry::clear()),
        Some(l @ ("on" | "off" | "full" | "metrics")) => {
            match achuk_telemetry::set_level(l) {
                Ok(msg) => println!("{msg}"),
                Err(e) => anyhow::bail!(e),
            }
        }
        _ => println!("{}", achuk_telemetry::status()),
    }
    Ok(())
}

/// `achuk task-grammar <task.json>` — the GBNF grammar constraining
/// generation to the task's scope (the bench runner's A2 projection).
fn task_grammar_cmd(args: &[String]) -> anyhow::Result<()> {
    use achuk_bench_grader::Task;
    use achuk_constraint::{legal_continuations, HoleContext};
    let task: Task = serde_json::from_str(&std::fs::read_to_string(need(
        args,
        0,
        "path to task.json",
    )?)?)?;
    let cdb = task.build_scope_cdb()?;
    let hole = HoleContext {
        editing: None,
        expected: achuk_core::Type::Var("any".into()),
    };
    let mask = legal_continuations(&cdb, &hole)?;
    println!("{}", mask.to_gbnf());
    Ok(())
}

/// `achuk corpus gen` — emit a synthetic supervised-fine-tuning corpus
/// (JSONL) generated from the CDB's in-scope symbols. The cold-start seed.
fn corpus_gen_cmd(db_path: &Path, stdlib: bool) -> anyhow::Result<()> {
    let examples = if stdlib {
        achuk_corpus::generate_stdlib()?
    } else {
        let cdb = Cdb::open(db_path)?;
        achuk_corpus::generate(&cdb)?
    };
    if examples.is_empty() {
        anyhow::bail!(
            "no function symbols in {} — ingest or bind some, or use --stdlib",
            db_path.display()
        );
    }
    print!("{}", achuk_corpus::to_jsonl(&examples));
    println!();
    eprintln!("generated {} example(s)", examples.len());
    Ok(())
}

/// `achuk index [dir]` — ingest every `.achuk` file under a project into the
/// CDB, so `candidates`/`mask`/MCP answer over the user's real symbols.
/// Rebuilds the store fresh each run (idempotent).
fn index_cmd(db_path: &Path, args: &[String]) -> anyhow::Result<()> {
    let root = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root().unwrap_or_else(|| PathBuf::from(".")));
    let files = achuk_files(&root);
    anyhow::ensure!(!files.is_empty(), "no .achuk files under {}", root.display());

    // Fresh store each index — but package definitions (installed by
    // `achuk add`, marked by their doc provenance) must SURVIVE a re-index,
    // or `achuk mcp install` would silently unlearn every dependency.
    let mut preserved: Vec<(String, Def)> = Vec::new();
    if let Ok(old) = Cdb::open(db_path) {
        for (n, h) in old.symbols().unwrap_or_default() {
            if let Ok(d) = old.get(&h) {
                if d.doc.starts_with("from package ") {
                    preserved.push((n, d));
                }
            }
        }
    }
    let _ = std::fs::remove_file(db_path);
    let mut cdb = Cdb::open(db_path)?;
    for (n, d) in &preserved {
        if let Ok(h) = cdb.put(d) {
            let _ = cdb.bind(n, &h);
        }
    }
    if !preserved.is_empty() {
        eprintln!("  kept {} package definition(s)", preserved.len());
    }
    let (mut ok, mut total) = (0usize, 0usize);
    for f in &files {
        match ingest(&mut cdb, f) {
            Ok(n) => {
                total += n;
                ok += 1;
            }
            Err(e) => eprintln!("  skip {}: {e}", f.display()),
        }
    }
    // Link the call graph across all ingested files (by name).
    let edges = link_edges(&cdb).unwrap_or(0);
    eprintln!(
        "indexed {total} definition(s) from {ok}/{} file(s), {edges} edge(s) → {}",
        files.len(),
        db_path.display()
    );
    Ok(())
}

/// `achuk mcp install` — write a project-scoped `.mcp.json` so Claude Code
/// (and any MCP client that reads it) auto-connects the Achuk server, giving
/// the agent the real-symbol guardrail. Merges into an existing file.
fn mcp_install_cmd() -> anyhow::Result<()> {
    let root = project_root().unwrap_or_else(|| PathBuf::from("."));
    let cfg_path = root.join(".mcp.json");
    let mcp_bin = find_tool("achuk-mcp")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "achuk-mcp".into());

    // Merge into an existing .mcp.json rather than clobber it.
    let mut cfg: serde_json::Value = std::fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !cfg
        .get("mcpServers")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        cfg["mcpServers"] = serde_json::json!({});
    }
    cfg["mcpServers"]["achuk"] = serde_json::json!({
        "command": mcp_bin,
        "args": ["--db", "achuk.cdb"],
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)? + "\n")?;

    // Make sure the store the server reads actually exists.
    if project_root().is_some() {
        let _ = index_cmd(
            &root.join("achuk.cdb"),
            &[root.to_string_lossy().into_owned()],
        );
    }
    eprintln!("wrote {}", cfg_path.display());
    eprintln!("Claude Code will connect the `achuk` MCP server in this project.");
    eprintln!("Its tools (achuk_symbols/achuk_candidates/achuk_mask) answer over your real code.");
    Ok(())
}

/// Resolve a bundled platform directory by short name. Order: $ACHUK_PLATFORMS,
/// then the packaged layout (<bindir>/../platforms/<name>), then the dev
/// monorepo (compiler/test/<mapped>/platform).
fn find_platform(name: &str) -> anyhow::Result<PathBuf> {
    // dev monorepo mapping: short name → compiler test platform dir
    let mapped = match name {
        "http" => "http-headers",
        "cli" => "fx-open",
        other => anyhow::bail!("unknown platform `{other}` (try: http, cli)"),
    };
    if let Ok(root) = std::env::var("ACHUK_PLATFORMS") {
        let p = Path::new(&root).join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // packaged: bin/../platforms/<name>
        if let Some(bindir) = exe.parent() {
            let p = bindir.join("..").join("platforms").join(name);
            if p.exists() {
                return Ok(p);
            }
        }
        // dev: walk up to compiler/test/<mapped>/platform
        let mut dir = exe;
        while dir.pop() {
            let p = dir.join("compiler/test").join(mapped).join("platform");
            if p.exists() {
                return Ok(p);
            }
        }
    }
    anyhow::bail!("could not locate the `{name}` platform (set ACHUK_PLATFORMS)")
}

/// Recursively copy a directory.
fn copy_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// `achuk db eval --real <name> <args...>` — evaluate a def through the
/// ACTUAL compiler (Roc's real interpreter), not the built-in one. Locates
/// the def's source (recorded at ingest), builds a runner that prints
/// `name(args)`, and runs it with `achukc`. This is the ground-truth
/// evaluator; the built-in interp is the fast self-contained approximation.
fn eval_real(cdb: &Cdb, name: &str, call_args: &[String]) -> anyhow::Result<()> {
    // Recover the source file from the def's provenance ("ingested from …").
    let h = cdb
        .resolve(name)
        .map_err(|_| anyhow::anyhow!("no such def: {name}"))?;
    let def = cdb.get(&h)?;
    let file = def
        .doc
        .strip_prefix("ingested from ")
        .ok_or_else(|| anyhow::anyhow!("no source recorded for {name} (was it indexed?)"))?
        .trim();
    let source =
        std::fs::read_to_string(file).map_err(|_| anyhow::anyhow!("source file gone: {file}"))?;

    // Drop any existing `main!` so ours is the entry point.
    let trimmed = match source.find("\nmain!") {
        Some(i) => &source[..i],
        None => source.strip_prefix("main!").map(|_| "").unwrap_or(&source),
    };

    // Format each argument: integer as-is, Uppercase word as a bare tag,
    // anything else as a string literal.
    let call = format!(
        "{name}({})",
        call_args
            .iter()
            .map(|a| {
                if a.parse::<i64>().is_ok() || a.chars().next().is_some_and(|c| c.is_uppercase()) {
                    a.clone()
                } else {
                    format!("{a:?}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    );

    let runner = format!(
        "{trimmed}\n\nmain! = |_achuk_eval| {{\n    echo!(Str.inspect({call}))\n    Ok({{}})\n}}\n"
    );
    let tmp = std::env::temp_dir().join(format!("achuk-eval-{}.achuk", std::process::id()));
    std::fs::write(&tmp, runner)?;

    let out = std::process::Command::new(find_achukc()?)
        .arg(&tmp)
        .output()?;
    let _ = std::fs::remove_file(&tmp);
    // The program's printed output is the signal — the last non-empty
    // stdout line. (achukc exits non-zero merely for warnings, so the exit
    // code isn't a reliable success gate; a real compile error yields no
    // program output at all.)
    let stdout = String::from_utf8_lossy(&out.stdout);
    let result = stdout
        .replace('\r', "\n")
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string();
    anyhow::ensure!(
        !result.is_empty(),
        "real eval produced no output — a compile error?\n{}",
        String::from_utf8_lossy(&out.stderr)
            .replace('\r', "\n")
            .lines()
            .filter(|l| l.to_lowercase().contains("error") && !l.contains("0 error"))
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!("{result}");
    Ok(())
}

/// The registry base URL: $ACHUK_REGISTRY, else the local default.
fn registry_url() -> String {
    std::env::var("ACHUK_REGISTRY").unwrap_or_else(|_| "https://registry.achuk.dev".into())
}

/// Read a `key = "value"` from achuk.toml's [project] section.
fn toml_value(toml: &str, key: &str) -> Option<String> {
    toml.lines()
        .find_map(|l| l.trim().strip_prefix(key))
        .and_then(|v| v.trim().strip_prefix('='))
        .map(|v| v.trim().trim_matches('"').to_string())
}

/// `achuk publish [dir]` — bundle this package and upload it to the registry.
/// The stored registry token: $ACHUK_TOKEN, else ~/.achuk/token.
fn registry_token() -> Option<String> {
    if let Ok(t) = std::env::var("ACHUK_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    let p = dirs_home()?.join(".achuk").join("token");
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `achuk login <token>` — store the token from your registry account page
/// (https://registry.achuk.dev/u/<you>). Mirrors `cargo login`.
fn login_cmd(args: &[String]) -> anyhow::Result<()> {
    let token = match args.first() {
        Some(t) => t.trim().to_string(),
        None => {
            eprintln!("paste your token (from {}/login), then Enter:", registry_url());
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim().to_string()
        }
    };
    anyhow::ensure!(!token.is_empty(), "no token given");
    let dir = dirs_home().ok_or_else(|| anyhow::anyhow!("no HOME"))?.join(".achuk");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("token");
    std::fs::write(&path, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    println!("saved token to {}", path.display());
    Ok(())
}

fn publish_cmd(args: &[String]) -> anyhow::Result<()> {
    let root = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root().unwrap_or_else(|| PathBuf::from(".")));
    let toml = std::fs::read_to_string(root.join("achuk.toml"))
        .map_err(|_| anyhow::anyhow!("no achuk.toml in {}", root.display()))?;
    let name = toml_value(&toml, "name").ok_or_else(|| anyhow::anyhow!("achuk.toml has no name"))?;
    let version = toml_value(&toml, "version").unwrap_or_else(|| "0.1.0".into());
    let entry = toml_value(&toml, "entry").unwrap_or_else(|| "main.achuk".into());

    // Bundle: achukc bundle <entry> --output-dir <tmp>. The compiler names
    // the output <base58-blake3>.tar.zst (content-addressed).
    let outdir = std::env::temp_dir().join(format!("achuk-pub-{}", std::process::id()));
    std::fs::create_dir_all(&outdir)?;
    let status = std::process::Command::new(find_achukc()?)
        .arg("bundle")
        .arg(root.join(&entry))
        .arg("--output-dir")
        .arg(&outdir)
        .status()?;
    anyhow::ensure!(status.success(), "achukc bundle failed");
    let bundle = std::fs::read_dir(&outdir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|x| x.to_str()) == Some("zst"))
        .ok_or_else(|| anyhow::anyhow!("no .tar.zst produced"))?;

    // MCP-compatibility contract: a package publishes WITH its definitions
    // (names, types, effects, docs) so every consumer's code database — and
    // therefore their AI — understands it the moment it is installed.
    let mut tmp_cdb = Cdb::in_memory()?;
    let n_defs = ingest(&mut tmp_cdb, &root.join(&entry))?;
    anyhow::ensure!(
        n_defs > 0,
        "`{entry}` exposes no definitions — a package must export at least one \
         (the registry requires MCP-compatible metadata)"
    );
    let mut defs = Vec::new();
    for (dname, h) in tmp_cdb.symbols()? {
        let d = tmp_cdb.get(&h)?;
        defs.push(serde_json::json!({
            "name": dname, "ty": d.ty.to_string(),
            "effects": d.effects, "doc": d.doc,
        }));
    }
    let defs_path = outdir.join("defs.json");
    std::fs::write(&defs_path, serde_json::to_vec(&defs)?)?;
    eprintln!("  {} definitions exported for the AI layer", defs.len());

    let reg = registry_url();
    let token = registry_token().ok_or_else(|| {
        anyhow::anyhow!("not logged in — run `achuk login <token>` (get one at {reg}/login)")
    })?;
    eprintln!("publishing {name}@{version} → {reg}");
    let out = std::process::Command::new("curl")
        .args(["-s", "-X", "POST", &format!("{reg}/publish")])
        .args(["-H", &format!("Authorization: Bearer {token}")])
        .args(["-F", &format!("name={name}")])
        .args(["-F", &format!("version={version}")])
        .arg("-F")
        .arg(format!("bundle=@{}", bundle.display()))
        .arg("-F")
        .arg(format!("defs=@{}", defs_path.display()))
        .output()?;
    let _ = std::fs::remove_dir_all(&outdir);
    anyhow::ensure!(out.status.success(), "upload failed");
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| anyhow::anyhow!("registry: {}", String::from_utf8_lossy(&out.stdout)))?;
    println!("published {name}@{version}");
    println!("  {}", resp["url"].as_str().unwrap_or("?"));
    Ok(())
}

/// `achuk add <name>[@version]` — add a registry dependency to this project:
/// records it in achuk.toml and inserts it into the app header so imports
/// resolve. The compiler fetches it on the next build/run.
fn add_cmd(args: &[String]) -> anyhow::Result<()> {
    let spec = need(args, 0, "package name")?;
    let (name, want_version) = match spec.split_once('@') {
        Some((n, v)) => (n.to_string(), Some(v.to_string())),
        None => (spec.clone(), None),
    };
    let root = project_root().unwrap_or_else(|| PathBuf::from("."));
    let reg = registry_url();

    // Look the package up in the registry.
    let out = std::process::Command::new("curl")
        .args(["-s", &format!("{reg}/packages/{name}")])
        .output()?;
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| anyhow::anyhow!("`{name}` not found in registry {reg}"))?;
    // pick the requested version, else latest
    let (version, url) = if let Some(v) = &want_version {
        let entry = meta["versions"]
            .as_array()
            .and_then(|vs| vs.iter().find(|e| e["version"].as_str() == Some(v)))
            .ok_or_else(|| anyhow::anyhow!("{name}@{v} not in registry"))?;
        (v.clone(), entry["url"].as_str().unwrap_or("").to_string())
    } else {
        let l = &meta["latest"];
        (
            l["version"].as_str().unwrap_or("").to_string(),
            l["url"].as_str().unwrap_or("").to_string(),
        )
    };
    anyhow::ensure!(!url.is_empty(), "registry returned no url for {name}");

    // Record in achuk.toml [dependencies].
    let toml_path = root.join("achuk.toml");
    let mut toml = std::fs::read_to_string(&toml_path).unwrap_or_default();
    if !toml.contains("[dependencies]") {
        toml.push_str("\n[dependencies]\n");
    }
    // Pull the package's definitions into the project's code database:
    // from this moment `achuk db candidates`, the MCP server, and `achuk ai`
    // all know the package's names, types, and effects.
    let defs_out = std::process::Command::new("curl")
        .args(["-s", &format!("{reg}/defs/{name}/{version}")])
        .output()?;
    if let Ok(pkg_defs) = serde_json::from_slice::<Vec<serde_json::Value>>(&defs_out.stdout) {
        let mut cdb = Cdb::open(&root.join("achuk.cdb"))?;
        let mut added = 0usize;
        for d in &pkg_defs {
            let (Some(dn), Some(ts)) = (d["name"].as_str(), d["ty"].as_str()) else { continue };
            let Ok(ty) = achuk_core::parse::parse_type(ts) else { continue };
            let mut def = Def::new(
                achuk_core::Expr::Lit(achuk_core::Lit::Str(format!("{name}::{dn}"))),
                ty,
            );
            def.effects = d["effects"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(String::from)).collect()).unwrap_or_default();
            def.doc = format!("from package {name}@{version}. {}", d["doc"].as_str().unwrap_or(""));
            if cdb.resolve(dn).is_err() {
                let h = cdb.put(&def)?;
                cdb.bind(dn, &h)?;
                added += 1;
            }
        }
        eprintln!("  {added} definitions added to the code database (AI-visible)");
    } else {
        eprintln!("  warning: package has no defs metadata — pre-MCP package; the AI won't see inside it");
    }

    let dep_line = format!("{name} = {{ version = \"{version}\", url = \"{url}\" }}\n");
    if let Some(pos) = toml.find("[dependencies]") {
        let insert_at = toml[pos..]
            .find('\n')
            .map(|i| pos + i + 1)
            .unwrap_or(toml.len());
        // drop any existing line for this package, then insert
        let mut kept: Vec<&str> = toml.lines().collect();
        kept.retain(|l| !l.trim_start().starts_with(&format!("{name} =")));
        toml = kept.join("\n");
        if !toml.ends_with('\n') {
            toml.push('\n');
        }
        let _ = insert_at;
    }
    if !toml.contains(&format!("{name} = {{")) {
        toml.push_str(&dep_line);
    }
    std::fs::write(&toml_path, &toml)?;

    // Insert the package into the app header so `import {name}.X` resolves.
    let entry = toml_value(&toml, "entry").unwrap_or_else(|| "main.achuk".into());
    let entry_path = root.join(&entry);
    if let Ok(src) = std::fs::read_to_string(&entry_path) {
        if src.contains("app [") && !src.contains(&format!("{name}:")) {
            // insert after the first `{` of the header record
            if let Some(brace) = src.find('{') {
                let (head, tail) = src.split_at(brace + 1);
                let patched = format!("{head}\n    {name}: \"{url}\",{tail}");
                std::fs::write(&entry_path, patched)?;
            }
        }
    }

    println!("added {name}@{version}");
    println!("  import {name}.<Module> to use it — `achuk run` fetches it");
    Ok(())
}

/// Find the project root (nearest ancestor with `achuk.toml`) or None.
fn project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("achuk.toml").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// All `.achuk` files under `root` (recursive, skipping hidden/dist dirs).
fn achuk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                if !name.starts_with('.') && name != "dist" && name != "target" {
                    stack.push(p);
                }
            } else if p.extension().and_then(|x| x.to_str()) == Some("achuk") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Locate a vendored compiler tool binary. Order: $ACHUK_COMPILER (for
/// achukc only), then the monorepo default (compiler/zig-out/bin/<tool>
/// walking up from this binary), then PATH.
fn find_tool(tool: &str) -> anyhow::Result<PathBuf> {
    if tool == "achukc" {
        if let Ok(p) = std::env::var("ACHUK_COMPILER") {
            return Ok(PathBuf::from(p));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // Packaged install: tools sit next to `achuk` (e.g. ~/.achuk/bin/achukc).
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(tool);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
        // Monorepo/dev: walk up to compiler/zig-out/bin/<tool>.
        let mut dir = exe;
        while dir.pop() {
            let candidate = dir.join(format!("compiler/zig-out/bin/{tool}"));
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    Ok(PathBuf::from(tool))
}

fn find_achukc() -> anyhow::Result<PathBuf> {
    find_tool("achukc")
}

/// Reverse index hash → bound name, for human-readable graph output.
fn hash_names(cdb: &Cdb) -> anyhow::Result<std::collections::HashMap<String, String>> {
    Ok(cdb.symbols()?.into_iter().map(|(n, h)| (h.0, n)).collect())
}

fn label_hash(names: &std::collections::HashMap<String, String>, h: &Hash) -> String {
    match names.get(&h.0) {
        Some(name) => format!("{name}  {h}"),
        None => h.0.clone(),
    }
}

/// A `Resolver` backed by the CDB: resolves references (by hash) and free
/// names (top-level defs) to their bodies, so the interpreter can run
/// lowered definitions straight from the database.
struct CdbResolver<'a> {
    cdb: &'a Cdb,
    /// hash -> name, built once — eval calls name_of per Ref, and a full
    /// symbols() table scan per call is quadratic on larger DBs.
    names: std::collections::HashMap<Hash, String>,
}

impl<'a> CdbResolver<'a> {
    fn new(cdb: &'a Cdb) -> Self {
        let names = cdb
            .symbols()
            .map(|v| v.into_iter().map(|(n, h)| (h, n)).collect())
            .unwrap_or_default();
        CdbResolver { cdb, names }
    }
}

impl achuk_core::interp::Resolver for CdbResolver<'_> {
    fn resolve(&self, h: &Hash) -> Option<achuk_core::Expr> {
        self.cdb.get(h).ok().map(|d| d.expr)
    }
    fn name_of(&self, h: &Hash) -> Option<String> {
        self.names.get(h).cloned()
    }
    fn resolve_name(&self, name: &str) -> Option<achuk_core::Expr> {
        let h = self.cdb.resolve(name).ok()?;
        self.cdb.get(&h).ok().map(|d| d.expr)
    }
}

/// Parse a `achuk db eval` argument: an integer, or a bare Uppercase word as
/// a nullary tag (for tag-union inputs like a pipeline `Lead` stage).
fn parse_eval_arg(a: &str) -> achuk_core::Expr {
    use achuk_core::{Expr, Lit};
    if let Ok(n) = a.parse::<i64>() {
        Expr::Lit(Lit::Int(n))
    } else if a.chars().next().is_some_and(|c| c.is_uppercase()) {
        Expr::Tag(a.to_string(), vec![])
    } else {
        Expr::Lit(Lit::Str(a.to_string()))
    }
}

/// Convert an interpreter value to a contract value (same shape).
fn to_contract_value(v: &achuk_core::interp::Value) -> achuk_contract::Value {
    use achuk_contract::Value as C;
    use achuk_core::interp::Value as I;
    match v {
        I::Int(n) => C::Int(*n),
        I::Bool(b) => C::Bool(*b),
        I::Str(s) => C::Str(s.clone()),
        I::List(xs) => C::List(xs.iter().map(to_contract_value).collect()),
        I::Ok(x) => C::Ok(Box::new(to_contract_value(x))),
        I::Err(x) => C::Err(Box::new(to_contract_value(x))),
        // closures/builtins aren't contract values; represent opaquely
        other => C::Str(format!("{other:?}")),
    }
}

/// Human-readable rendering of an interpreter value.
fn fmt_value(v: &achuk_core::interp::Value) -> String {
    use achuk_core::interp::Value;
    match v {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => format!("{s:?}"),
        Value::Ok(x) => format!("Ok({})", fmt_value(x)),
        Value::Err(x) => format!("Err({})", fmt_value(x)),
        Value::Tag(name, args) if args.is_empty() => name.clone(),
        Value::Tag(name, args) => {
            let a: Vec<String> = args.iter().map(fmt_value).collect();
            format!("{name}({})", a.join(", "))
        }
        Value::Record(m) => {
            let fs: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k}: {}", fmt_value(v)))
                .collect();
            format!("{{ {} }}", fs.join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// Resolve a CLI reference: try as bound name first, then as full hash.
fn resolve_ref(cdb: &Cdb, r: &str) -> anyhow::Result<Hash> {
    if let Ok(h) = cdb.resolve(r) {
        return Ok(h);
    }
    let h = Hash(r.trim_start_matches('#').to_string());
    anyhow::ensure!(
        cdb.contains_hash(&h)?,
        "`{r}` is neither a bound name nor a known hash"
    );
    Ok(h)
}

fn db_cmd(db_path: &Path, args: &[String]) -> anyhow::Result<()> {
    let mut cdb = Cdb::open(db_path)?;
    match args.first().map(String::as_str) {
        Some("symbols") => {
            for (name, hash) in cdb.symbols()? {
                let def = cdb.get(&hash)?;
                println!("{name} : {}  {hash}", def.ty);
            }
        }
        Some("put") => {
            let mut raw = String::new();
            std::io::stdin().read_to_string(&mut raw)?;
            let def: Def = serde_json::from_str(&raw)?;
            let hash = cdb.put(&def)?;
            println!("{}", hash.0);
        }
        Some("bind") => {
            let (name, hash) = (need(args, 1, "name")?, need(args, 2, "hash")?);
            cdb.bind(name, &Hash(hash.trim_start_matches('#').to_string()))?;
        }
        Some("resolve") => {
            println!("{}", cdb.resolve(need(args, 1, "name")?)?.0);
        }
        Some("candidates") => {
            let ty = parse_type(need(args, 1, "type signature")?)?;
            for c in cdb.candidates(&ty)? {
                let dep = if c.deprecated { "  [deprecated]" } else { "" };
                println!("{} : {}  {}{dep}", c.name, c.ty, c.hash);
            }
        }
        Some("callers") => {
            let h = resolve_ref(&cdb, need(args, 1, "name|hash")?)?;
            let names = hash_names(&cdb)?;
            for caller in cdb.callers(&h)? {
                println!("{}", label_hash(&names, &caller));
            }
        }
        Some("deps") => {
            let h = resolve_ref(&cdb, need(args, 1, "name|hash")?)?;
            let names = hash_names(&cdb)?;
            for dep in cdb.deps(&h)? {
                println!("{}", label_hash(&names, &dep));
            }
        }
        Some("render") => {
            let name_or_hash = need(args, 1, "name|hash")?;
            let h = resolve_ref(&cdb, name_or_hash)?;
            let def = cdb.get(&h)?;
            // `--achuk` renders the human-readable .achuk projection; default = JSON.
            if args.iter().any(|a| a == "--achuk") {
                let name = cdb
                    .resolve(name_or_hash)
                    .ok()
                    .and(Some(name_or_hash.clone()));
                println!(
                    "{}",
                    achuk_core::render::render_def(name.as_deref().unwrap_or("_"), &def)
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&def)?);
            }
        }
        Some("ingest") => {
            let file = need(args, 1, "path to .achuk file")?;
            let n = ingest(&mut cdb, Path::new(file))?;
            eprintln!("ingested {n} definition(s) from {file}");
        }
        // Property-check a real def: run it on every integer input in a
        // range and verify a postcondition. Contracts, on your actual code.
        //   achuk db check <name> <lo> <hi> "<ensures predicate>"
        // The predicate may reference `result` and the function's parameter.
        Some("check") => {
            use achuk_core::interp::Resolver as _;
            use achuk_core::{interp, Expr, Lit};
            let name = need(args, 1, "def name")?;
            let lo: i64 = need(args, 2, "lo")?.parse()?;
            let hi: i64 = need(args, 3, "hi")?.parse()?;
            let pred_src = need(args, 4, "ensures predicate")?;
            let pred = achuk_contract::parse_pred(pred_src).map_err(|e| anyhow::anyhow!("{e:?}"))?;

            let res = CdbResolver::new(&cdb);
            let body = res
                .resolve_name(name)
                .ok_or_else(|| anyhow::anyhow!("no such def: {name}"))?;
            let param = match &body {
                Expr::Lam { params, .. } if params.len() == 1 => params[0].clone(),
                _ => anyhow::bail!("`check` supports unary functions only"),
            };

            let mut failures = 0;
            for x in lo..=hi {
                let call = Expr::App {
                    func: Box::new(Expr::Var(name.clone())),
                    args: vec![Expr::Lit(Lit::Int(x))],
                };
                let result = match interp::eval(&call, &interp::Env::new(), &res) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("  {param}={x}: eval error {e:?}");
                        failures += 1;
                        continue;
                    }
                };
                let mut env: std::collections::BTreeMap<String, achuk_contract::Value> =
                    std::collections::BTreeMap::new();
                env.insert(param.clone(), to_contract_value(&interp::Value::Int(x)));
                env.insert("result".into(), to_contract_value(&result));
                match achuk_contract::eval_pred(&pred, &env) {
                    Ok(true) => {}
                    Ok(false) => {
                        println!(
                            "  counterexample: {param}={x} → result={}",
                            fmt_value(&result)
                        );
                        failures += 1;
                    }
                    Err(e) => {
                        println!("  {param}={x}: predicate error {e:?}");
                        failures += 1;
                    }
                }
            }
            if failures == 0 {
                println!("ok — `{name}` satisfies the contract for {param} in [{lo}, {hi}]");
            } else {
                println!("FAILED — {failures} counterexample(s)");
                std::process::exit(1);
            }
        }
        // Run a lowered definition from the database. Default: the built-in
        // interpreter over the CDB's lowered AST (fast, self-contained).
        // `--real`: run it through the *actual compiler* — the ground-truth
        // evaluator — collapsing the interpreter/AST double (see the
        // inheritance audit). Needs the def's source file.
        Some("eval") if args.iter().any(|a| a == "--real") => {
            // positional args after `eval`, ignoring the --real flag
            let pos: Vec<String> = args[1..]
                .iter()
                .filter(|a| a.as_str() != "--real")
                .cloned()
                .collect();
            let name = pos
                .first()
                .ok_or_else(|| anyhow::anyhow!("missing def name"))?;
            return eval_real(&cdb, name, &pos[1..]);
        }
        Some("eval") => {
            use achuk_core::{interp, Expr};
            let name = need(args, 1, "def name")?;
            // Each arg: an integer literal, or a bare Uppercase word → a
            // nullary tag (e.g. `Lead`), so tag-union state machines run.
            let call = Expr::App {
                func: Box::new(Expr::Var(name.clone())),
                args: args[2..].iter().map(|a| parse_eval_arg(a)).collect(),
            };
            let res = CdbResolver::new(&cdb);
            match interp::eval(&call, &interp::Env::new(), &res) {
                Ok(v) => println!("{}", fmt_value(&v)),
                Err(e) => {
                    eprintln!("eval error: {e:?}");
                    std::process::exit(1);
                }
            }
        }
        Some("mask") => {
            let ty = parse_type(need(args, 1, "type signature")?)?;
            let hole = HoleContext {
                editing: None,
                expected: ty,
            };
            match legal_continuations(&cdb, &hole)? {
                Mask::Symbols(list) => {
                    for c in &list {
                        println!("{} : {}", c.name, c.ty);
                    }
                    eprintln!("--- gbnf ---");
                    eprintln!("{}", achuk_constraint::gbnf::def_json_grammar(&list));
                }
                Mask::EmptyWithDiagnostic(d) => {
                    eprintln!("{}", d.render());
                    std::process::exit(1);
                }
            }
        }
        other => anyhow::bail!("unknown db subcommand {other:?}"),
    }
    Ok(())
}

fn need<'a>(args: &'a [String], i: usize, what: &str) -> anyhow::Result<&'a String> {
    args.get(i)
        .ok_or_else(|| anyhow::anyhow!("missing argument: {what}"))
}

// ---------------------------------------------------------------------
// Ingest: compiler -> CDB bridge
// ---------------------------------------------------------------------
// Runs `achukc defs --json <file>`, which canonicalizes + typechecks the
// file and emits a JSON array of {name, type, effectful} for each
// top-level def. Real names + inferred types + effect flag, structured —
// no fragile markdown scraping. Types the prototype signature parser can't
// express yet (records, tags) fall back to an opaque Named(raw), still
// queryable by identity. Bodies stay a placeholder until real body-lowering
// lands (the AST can now hold them; the compiler->AST lowering is next).

fn ingest(cdb: &mut Cdb, file: &Path) -> anyhow::Result<usize> {
    use achuk_core::{Expr, Lit, Type};

    #[derive(serde::Deserialize)]
    struct DefEntry {
        name: String,
        #[serde(rename = "type")]
        ty: String,
        #[serde(default)]
        effectful: bool,
        /// The lowered body, if the compiler emitted one (references to
        /// other defs are `Var(name)`; edges are linked by name afterward).
        #[serde(default)]
        body: Option<Expr>,
    }

    let achukc = find_achukc()?;
    let out = std::process::Command::new(&achukc)
        .arg("defs")
        .arg("--json")
        .arg(file)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "achukc defs failed for {}: {}",
        file.display(),
        String::from_utf8_lossy(&out.stderr)
    );

    // The JSON array is emitted on its own line; take the last `[`-prefixed
    // line (the compiler may print diagnostics before it).
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('['))
        .unwrap_or("[]")
        .trim();
    let entries: Vec<DefEntry> =
        serde_json::from_str(json).map_err(|e| anyhow::anyhow!("parsing achukc defs json: {e}"))?;
    anyhow::ensure!(
        !entries.is_empty(),
        "no top-level definitions found in {}",
        file.display()
    );

    let mut count = 0;
    for e in &entries {
        // Prototype signature parser first; opaque fallback keeps ingest total.
        let ty = parse_type(&e.ty).unwrap_or_else(|_| Type::Named(e.ty.clone()));
        // Effect row: the compiler's effectful flag becomes a visible effect.
        let effects = if e.effectful {
            vec!["Effect".to_string()]
        } else {
            vec![]
        };
        // Use the compiler-lowered body when present; else an opaque marker.
        let body = e
            .body
            .clone()
            .unwrap_or_else(|| Expr::Lit(Lit::Str(format!("{}::{}", file.display(), e.name))));
        let def = Def {
            expr: body,
            ty,
            effects,
            deprecated: false,
            doc: format!("ingested from {}", file.display()),
        };
        let h = cdb.put(&def)?;
        cdb.bind(&e.name, &h)?;
        count += 1;
    }
    Ok(count)
}

/// Build call-graph edges over everything currently in the CDB: for each
/// bound def, any free variable in its body that names another bound def
/// becomes an edge (caller → callee). Bodies reference deps by name, so
/// this resolves them to hashes — making `deps`/`callers` work on real,
/// mutually-recursive code without content-hashing cycles.
fn link_edges(cdb: &Cdb) -> anyhow::Result<usize> {
    let names: std::collections::HashMap<String, Hash> = cdb.symbols()?.into_iter().collect();
    let mut edges = 0;
    for (name, caller_hash) in cdb.symbols()? {
        let def = cdb.get(&caller_hash)?;
        for free in def.expr.free_vars() {
            if free == name {
                continue; // self-reference (recursion) isn't a dependency edge
            }
            if let Some(callee_hash) = names.get(&free) {
                cdb.add_edge(&caller_hash, callee_hash)?;
                edges += 1;
            }
        }
    }
    Ok(edges)
}
