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
            for caller in cdb.callers(&h)? {
                println!("{}", caller.0);
            }
        }
        Some("deps") => {
            let h = resolve_ref(&cdb, need(args, 1, "name|hash")?)?;
            for dep in cdb.deps(&h)? {
                println!("{}", dep.0);
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
