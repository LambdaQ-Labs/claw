//! claw — the Claw toolchain CLI (WS-I).
//!
//! MVP surface: the code-as-database commands (docs/p2-spec.md §1.6).
//! The compiler subcommands (`claw build`, `claw check`) attach here once
//! the vendored compiler is wired up.
//!
//!   claw db symbols                          list bound names
//!   claw db put < def.json                   insert a definition (stdin)
//!   claw db bind <name> <hash>               point a name at a hash
//!   claw db resolve <name>                   name -> hash
//!   claw db candidates "<type sig>"          type-directed symbol query
//!   claw db callers <name|hash>              who references this
//!   claw db deps <name|hash>                 what this references
//!   claw db render <name|hash>               definition as JSON
//!   claw db mask "<type sig>"                legal continuations + GBNF
//!
//! Store path: --db <file> (default ./claw.cdb).

use claw_cdb::Cdb;
use claw_constraint::{legal_continuations, HoleContext, Mask};
use claw_core::{parse::parse_type, Def, Hash};
use std::io::Read;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // extract --db <path> anywhere in the argv
    let mut db_path = PathBuf::from("claw.cdb");
    if let Some(i) = args.iter().position(|a| a == "--db") {
        anyhow::ensure!(i + 1 < args.len(), "--db needs a value");
        db_path = PathBuf::from(args.remove(i + 1));
        args.remove(i);
    }

    match args.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            println!("claw {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("db") => db_cmd(&db_path, &args[1..]),
        // Project model.
        Some("new") => new_cmd(&args[1..]),
        Some("run") => run_cmd(&args[1..]),
        // Compiler passthrough: `claw check|build|fmt|test|repl <args>` runs
        // the vendored compiler (clawc). CLAW_COMPILER overrides discovery.
        Some(cmd @ ("check" | "build" | "fmt" | "test" | "repl")) => {
            let status = std::process::Command::new(find_clawc()?)
                .arg(cmd)
                .args(&args[1..])
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        // WS-G: transpile a Def-JSON file (the benchmark protocol) to Rust.
        Some("emit-rust") => emit_rust_cmd(&args[1..]),
        // WS-J: real-compiler compile signal — render Def-JSON + task scope
        // as a .claw module and run `clawc check` on it. `--batch` grades an
        // outputs.jsonl ({"task": <file>, "defs": [...]} per line).
        Some("defs-check") => defs_check_cmd(&args[1..]),
        // WS-H: generate a synthetic SFT corpus (JSONL). `--stdlib` uses the
        // built-in stdlib scope; otherwise reads the CDB at --db.
        Some("corpus") if args.get(1).map(String::as_str) == Some("gen") => {
            corpus_gen_cmd(&db_path, args.iter().any(|a| a == "--stdlib"))
        }
        // Index a whole project's .claw files into the CDB so the AI
        // guardrail (candidates/mask/MCP) answers over the user's real code.
        Some("index") => index_cmd(&db_path, &args[1..]),
        // Register the MCP server with an agent (Claude Code) so it writes
        // Claw grounded in the project's real symbols.
        Some("mcp") if args.get(1).map(String::as_str) == Some("install") => mcp_install_cmd(),
        // Package manager: publish this package to the registry, or add a
        // dependency from the registry to this project.
        Some("publish") => publish_cmd(&args[1..]),
        Some("add") => add_cmd(&args[1..]),
        _ => {
            eprintln!(
                "claw — the Claw toolchain\n\nusage:\n  claw new <name>                              scaffold a new project\n  claw run [file.claw]                         run a program (default: main.claw)\n  claw build|check|fmt|test|repl <file.claw>   compiler passthrough\n  claw [--db <file>] db <subcommand>           code-as-database\n  claw emit-rust <defs.json>                    transpile Def-JSON → Rust\n  claw [--db <file>] corpus gen [--stdlib]      synthetic SFT corpus → JSONL\n\ndb subcommands:\n  symbols | put | bind <name> <hash> | resolve <name> | ingest <file.claw>\n  candidates \"<type>\" | callers <ref> | deps <ref> | render <ref> | mask \"<type>\""
            );
            std::process::exit(2);
        }
    }
}

/// `claw new <name> [--platform http|cli]` — scaffold a runnable project.
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
        .ok_or_else(|| anyhow::anyhow!("usage: claw new <name> [--platform http|cli]"))?;
    let dir = Path::new(name);
    anyhow::ensure!(!dir.exists(), "`{name}` already exists");
    std::fs::create_dir_all(dir)?;

    let (entry, source) = match platform.as_deref() {
        None => ("main.claw", DEFAULT_STARTER.to_string()),
        Some(p) => {
            // Copy the bundled platform into the project, generate an app.
            let src = find_platform(p)?;
            copy_dir(&src, &dir.join("platform"))?;
            (
                "app.claw",
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
        dir.join("claw.toml"),
        format!(
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"{entry}\"\nplatform = \"{}\"\n",
            platform.as_deref().unwrap_or("print")
        ),
    )?;
    std::fs::write(dir.join(".gitignore"), "/claw.cdb\n/dist\n*.o\n")?;
    std::fs::write(
        dir.join("README.md"),
        format!("# {name}\n\nA Claw project.\n\n```sh\nclaw run\n```\n"),
    )?;

    // Best-effort initial index so the AI guardrail works immediately.
    if let Ok(mut cdb) = Cdb::open(&dir.join("claw.cdb")) {
        let _ = ingest(&mut cdb, &dir.join(entry));
    }

    eprintln!("created project `{name}`");
    eprintln!("  cd {name} && claw run");
    Ok(())
}

const DEFAULT_STARTER: &str = "# Welcome to Claw. Run with `claw run`.\n\
    greet = |who| \"Hello, ${who}!\"\n\n\
    main! = |_args| {\n    \
    echo!(greet(\"world\"))\n    \
    Ok({})\n\
    }\n";

const HTTP_STARTER: &str = "app [main!] { pf: platform \"./platform/main.roc\" }\n\n\
    # An HTTP handler. The host passes the raw request headers; return a U64.\n\
    # Run `claw run` — it prints the port it bound, then serves a request.\n\
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
    Stdout.line!(\"Hello from a Claw CLI app!\")\n    \
    Ok({})\n\
    }\n";

/// The project's entry file: an explicit arg, else `claw.toml`'s entry,
/// else `main.claw`. Searches up from the cwd for `claw.toml`.
fn entry_file(args: &[String]) -> PathBuf {
    if let Some(f) = args.first() {
        return PathBuf::from(f);
    }
    // walk up for claw.toml → use its dir + entry
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            let toml = dir.join("claw.toml");
            if toml.exists() {
                let entry = std::fs::read_to_string(&toml)
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .find_map(|l| l.trim().strip_prefix("entry ="))
                            .map(|v| v.trim().trim_matches('"').to_string())
                    })
                    .unwrap_or_else(|| "main.claw".into());
                return dir.join(entry);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    PathBuf::from("main.claw")
}

/// `claw run [file]` — run a program via the compiler (default: main.claw).
fn run_cmd(args: &[String]) -> anyhow::Result<()> {
    let file = entry_file(args);
    anyhow::ensure!(file.exists(), "no such file: {}", file.display());
    let status = std::process::Command::new(find_clawc()?)
        .arg(&file)
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `claw emit-rust <defs.json>` — read a JSON array of named definitions
/// (the benchmark's Def-JSON protocol) and print a Rust module.
fn emit_rust_cmd(args: &[String]) -> anyhow::Result<()> {
    use claw_emit_rust::{emit_fn, NameMap};
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

    println!("// generated by `claw emit-rust` — do not edit");
    for (i, d) in defs.iter().enumerate() {
        let name = d.name.clone().unwrap_or_else(|| format!("def{i}"));
        match emit_fn(&name, &d.def, &names) {
            Ok(rust) => println!("\n{rust}"),
            Err(e) => eprintln!("// skipped {name}: {e}"),
        }
    }
    Ok(())
}

/// `claw defs-check <defs.json> <task.json>` (or `--batch <outputs.jsonl>`)
/// — the REAL compile signal: render the task's scope as signature-true
/// crash-stubs plus the produced defs, and run `clawc check` on the module.
fn defs_check_cmd(args: &[String]) -> anyhow::Result<()> {
    use claw_bench_grader::{realc, ProducedDef, Task};
    if std::env::var("CLAW_CLAWC").is_err() {
        std::env::set_var("CLAW_CLAWC", find_clawc()?);
    }

    let check_one = |task: &Task, defs: &[ProducedDef]| -> anyhow::Result<realc::RealCheck> {
        let module = realc::task_module(&task.scope, defs)?;
        realc::clawc_check(&module)
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
    if r.compiled {
        println!("COMPILE-OK");
    } else {
        println!("COMPILE-FAIL ({} errors)\n{}", r.errors, r.detail);
    }
    Ok(())
}

/// `claw corpus gen` — emit a synthetic supervised-fine-tuning corpus
/// (JSONL) generated from the CDB's in-scope symbols. The cold-start seed.
fn corpus_gen_cmd(db_path: &Path, stdlib: bool) -> anyhow::Result<()> {
    let examples = if stdlib {
        claw_corpus::generate_stdlib()?
    } else {
        let cdb = Cdb::open(db_path)?;
        claw_corpus::generate(&cdb)?
    };
    if examples.is_empty() {
        anyhow::bail!(
            "no function symbols in {} — ingest or bind some, or use --stdlib",
            db_path.display()
        );
    }
    print!("{}", claw_corpus::to_jsonl(&examples));
    println!();
    eprintln!("generated {} example(s)", examples.len());
    Ok(())
}

/// `claw index [dir]` — ingest every `.claw` file under a project into the
/// CDB, so `candidates`/`mask`/MCP answer over the user's real symbols.
/// Rebuilds the store fresh each run (idempotent).
fn index_cmd(db_path: &Path, args: &[String]) -> anyhow::Result<()> {
    let root = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root().unwrap_or_else(|| PathBuf::from(".")));
    let files = claw_files(&root);
    anyhow::ensure!(!files.is_empty(), "no .claw files under {}", root.display());

    // Fresh store each index.
    let _ = std::fs::remove_file(db_path);
    let mut cdb = Cdb::open(db_path)?;
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

/// `claw mcp install` — write a project-scoped `.mcp.json` so Claude Code
/// (and any MCP client that reads it) auto-connects the Claw server, giving
/// the agent the real-symbol guardrail. Merges into an existing file.
fn mcp_install_cmd() -> anyhow::Result<()> {
    let root = project_root().unwrap_or_else(|| PathBuf::from("."));
    let cfg_path = root.join(".mcp.json");
    let mcp_bin = find_tool("claw-mcp")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "claw-mcp".into());

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
    cfg["mcpServers"]["claw"] = serde_json::json!({
        "command": mcp_bin,
        "args": ["--db", "claw.cdb"],
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)? + "\n")?;

    // Make sure the store the server reads actually exists.
    if project_root().is_some() {
        let _ = index_cmd(
            &root.join("claw.cdb"),
            &[root.to_string_lossy().into_owned()],
        );
    }
    eprintln!("wrote {}", cfg_path.display());
    eprintln!("Claude Code will connect the `claw` MCP server in this project.");
    eprintln!("Its tools (claw_symbols/claw_candidates/claw_mask) answer over your real code.");
    Ok(())
}

/// Resolve a bundled platform directory by short name. Order: $CLAW_PLATFORMS,
/// then the packaged layout (<bindir>/../platforms/<name>), then the dev
/// monorepo (compiler/test/<mapped>/platform).
fn find_platform(name: &str) -> anyhow::Result<PathBuf> {
    // dev monorepo mapping: short name → compiler test platform dir
    let mapped = match name {
        "http" => "http-headers",
        "cli" => "fx-open",
        other => anyhow::bail!("unknown platform `{other}` (try: http, cli)"),
    };
    if let Ok(root) = std::env::var("CLAW_PLATFORMS") {
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
    anyhow::bail!("could not locate the `{name}` platform (set CLAW_PLATFORMS)")
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

/// `claw db eval --real <name> <args...>` — evaluate a def through the
/// ACTUAL compiler (Roc's real interpreter), not the built-in one. Locates
/// the def's source (recorded at ingest), builds a runner that prints
/// `name(args)`, and runs it with `clawc`. This is the ground-truth
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
        "{trimmed}\n\nmain! = |_claw_eval| {{\n    echo!(Str.inspect({call}))\n    Ok({{}})\n}}\n"
    );
    let tmp = std::env::temp_dir().join(format!("claw-eval-{}.claw", std::process::id()));
    std::fs::write(&tmp, runner)?;

    let out = std::process::Command::new(find_clawc()?)
        .arg(&tmp)
        .output()?;
    let _ = std::fs::remove_file(&tmp);
    // The program's printed output is the signal — the last non-empty
    // stdout line. (clawc exits non-zero merely for warnings, so the exit
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

/// The registry base URL: $CLAW_REGISTRY, else the local default.
fn registry_url() -> String {
    std::env::var("CLAW_REGISTRY").unwrap_or_else(|_| "http://127.0.0.1:8888".into())
}

/// Read a `key = "value"` from claw.toml's [project] section.
fn toml_value(toml: &str, key: &str) -> Option<String> {
    toml.lines()
        .find_map(|l| l.trim().strip_prefix(key))
        .and_then(|v| v.trim().strip_prefix('='))
        .map(|v| v.trim().trim_matches('"').to_string())
}

/// `claw publish [dir]` — bundle this package and upload it to the registry.
fn publish_cmd(args: &[String]) -> anyhow::Result<()> {
    let root = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root().unwrap_or_else(|| PathBuf::from(".")));
    let toml = std::fs::read_to_string(root.join("claw.toml"))
        .map_err(|_| anyhow::anyhow!("no claw.toml in {}", root.display()))?;
    let name = toml_value(&toml, "name").ok_or_else(|| anyhow::anyhow!("claw.toml has no name"))?;
    let version = toml_value(&toml, "version").unwrap_or_else(|| "0.1.0".into());
    let entry = toml_value(&toml, "entry").unwrap_or_else(|| "main.claw".into());

    // Bundle: clawc bundle <entry> --output-dir <tmp>. The compiler names
    // the output <base58-blake3>.tar.zst (content-addressed).
    let outdir = std::env::temp_dir().join(format!("claw-pub-{}", std::process::id()));
    std::fs::create_dir_all(&outdir)?;
    let status = std::process::Command::new(find_clawc()?)
        .arg("bundle")
        .arg(root.join(&entry))
        .arg("--output-dir")
        .arg(&outdir)
        .status()?;
    anyhow::ensure!(status.success(), "clawc bundle failed");
    let bundle = std::fs::read_dir(&outdir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|x| x.to_str()) == Some("zst"))
        .ok_or_else(|| anyhow::anyhow!("no .tar.zst produced"))?;

    let reg = registry_url();
    eprintln!("publishing {name}@{version} → {reg}");
    let out = std::process::Command::new("curl")
        .args(["-s", "-X", "POST", &format!("{reg}/publish")])
        .args(["-F", &format!("name={name}")])
        .args(["-F", &format!("version={version}")])
        .arg("-F")
        .arg(format!("bundle=@{}", bundle.display()))
        .output()?;
    let _ = std::fs::remove_dir_all(&outdir);
    anyhow::ensure!(out.status.success(), "upload failed");
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| anyhow::anyhow!("registry: {}", String::from_utf8_lossy(&out.stdout)))?;
    println!("published {name}@{version}");
    println!("  {}", resp["url"].as_str().unwrap_or("?"));
    Ok(())
}

/// `claw add <name>[@version]` — add a registry dependency to this project:
/// records it in claw.toml and inserts it into the app header so imports
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

    // Record in claw.toml [dependencies].
    let toml_path = root.join("claw.toml");
    let mut toml = std::fs::read_to_string(&toml_path).unwrap_or_default();
    if !toml.contains("[dependencies]") {
        toml.push_str("\n[dependencies]\n");
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
    let entry = toml_value(&toml, "entry").unwrap_or_else(|| "main.claw".into());
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
    println!("  import {name}.<Module> to use it — `claw run` fetches it");
    Ok(())
}

/// Find the project root (nearest ancestor with `claw.toml`) or None.
fn project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("claw.toml").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// All `.claw` files under `root` (recursive, skipping hidden/dist dirs).
fn claw_files(root: &Path) -> Vec<PathBuf> {
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
            } else if p.extension().and_then(|x| x.to_str()) == Some("claw") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Locate a vendored compiler tool binary. Order: $CLAW_COMPILER (for
/// clawc only), then the monorepo default (compiler/zig-out/bin/<tool>
/// walking up from this binary), then PATH.
fn find_tool(tool: &str) -> anyhow::Result<PathBuf> {
    if tool == "clawc" {
        if let Ok(p) = std::env::var("CLAW_COMPILER") {
            return Ok(PathBuf::from(p));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // Packaged install: tools sit next to `claw` (e.g. ~/.claw/bin/clawc).
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

fn find_clawc() -> anyhow::Result<PathBuf> {
    find_tool("clawc")
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

impl claw_core::interp::Resolver for CdbResolver<'_> {
    fn resolve(&self, h: &Hash) -> Option<claw_core::Expr> {
        self.cdb.get(h).ok().map(|d| d.expr)
    }
    fn name_of(&self, h: &Hash) -> Option<String> {
        self.names.get(h).cloned()
    }
    fn resolve_name(&self, name: &str) -> Option<claw_core::Expr> {
        let h = self.cdb.resolve(name).ok()?;
        self.cdb.get(&h).ok().map(|d| d.expr)
    }
}

/// Parse a `claw db eval` argument: an integer, or a bare Uppercase word as
/// a nullary tag (for tag-union inputs like a pipeline `Lead` stage).
fn parse_eval_arg(a: &str) -> claw_core::Expr {
    use claw_core::{Expr, Lit};
    if let Ok(n) = a.parse::<i64>() {
        Expr::Lit(Lit::Int(n))
    } else if a.chars().next().is_some_and(|c| c.is_uppercase()) {
        Expr::Tag(a.to_string(), vec![])
    } else {
        Expr::Lit(Lit::Str(a.to_string()))
    }
}

/// Convert an interpreter value to a contract value (same shape).
fn to_contract_value(v: &claw_core::interp::Value) -> claw_contract::Value {
    use claw_contract::Value as C;
    use claw_core::interp::Value as I;
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
fn fmt_value(v: &claw_core::interp::Value) -> String {
    use claw_core::interp::Value;
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
            // `--claw` renders the human-readable .claw projection; default = JSON.
            if args.iter().any(|a| a == "--claw") {
                let name = cdb
                    .resolve(name_or_hash)
                    .ok()
                    .and(Some(name_or_hash.clone()));
                println!(
                    "{}",
                    claw_core::render::render_def(name.as_deref().unwrap_or("_"), &def)
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&def)?);
            }
        }
        Some("ingest") => {
            let file = need(args, 1, "path to .claw file")?;
            let n = ingest(&mut cdb, Path::new(file))?;
            eprintln!("ingested {n} definition(s) from {file}");
        }
        // Property-check a real def: run it on every integer input in a
        // range and verify a postcondition. Contracts, on your actual code.
        //   claw db check <name> <lo> <hi> "<ensures predicate>"
        // The predicate may reference `result` and the function's parameter.
        Some("check") => {
            use claw_core::interp::Resolver as _;
            use claw_core::{interp, Expr, Lit};
            let name = need(args, 1, "def name")?;
            let lo: i64 = need(args, 2, "lo")?.parse()?;
            let hi: i64 = need(args, 3, "hi")?.parse()?;
            let pred_src = need(args, 4, "ensures predicate")?;
            let pred = claw_contract::parse_pred(pred_src).map_err(|e| anyhow::anyhow!("{e:?}"))?;

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
                let mut env: std::collections::BTreeMap<String, claw_contract::Value> =
                    std::collections::BTreeMap::new();
                env.insert(param.clone(), to_contract_value(&interp::Value::Int(x)));
                env.insert("result".into(), to_contract_value(&result));
                match claw_contract::eval_pred(&pred, &env) {
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
            use claw_core::{interp, Expr};
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
                    eprintln!("{}", claw_constraint::gbnf::def_json_grammar(&list));
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
// Runs `clawc defs --json <file>`, which canonicalizes + typechecks the
// file and emits a JSON array of {name, type, effectful} for each
// top-level def. Real names + inferred types + effect flag, structured —
// no fragile markdown scraping. Types the prototype signature parser can't
// express yet (records, tags) fall back to an opaque Named(raw), still
// queryable by identity. Bodies stay a placeholder until real body-lowering
// lands (the AST can now hold them; the compiler->AST lowering is next).

fn ingest(cdb: &mut Cdb, file: &Path) -> anyhow::Result<usize> {
    use claw_core::{Expr, Lit, Type};

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

    let clawc = find_clawc()?;
    let out = std::process::Command::new(&clawc)
        .arg("defs")
        .arg("--json")
        .arg(file)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "clawc defs failed for {}: {}",
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
        serde_json::from_str(json).map_err(|e| anyhow::anyhow!("parsing clawc defs json: {e}"))?;
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
