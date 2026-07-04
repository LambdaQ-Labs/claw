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
        _ => {
            eprintln!(
                "claw — the Claw toolchain\n\nusage:\n  claw new <name>                              scaffold a new project\n  claw run [file.claw]                         run a program (default: main.claw)\n  claw build|check|fmt|test|repl <file.claw>   compiler passthrough\n  claw [--db <file>] db <subcommand>           code-as-database\n  claw emit-rust <defs.json>                    transpile Def-JSON → Rust\n  claw [--db <file>] corpus gen [--stdlib]      synthetic SFT corpus → JSONL\n\ndb subcommands:\n  symbols | put | bind <name> <hash> | resolve <name> | ingest <file.claw>\n  candidates \"<type>\" | callers <ref> | deps <ref> | render <ref> | mask \"<type>\""
            );
            std::process::exit(2);
        }
    }
}

/// `claw new <name>` — scaffold a runnable project.
fn new_cmd(args: &[String]) -> anyhow::Result<()> {
    let name = need(args, 0, "project name")?;
    let dir = Path::new(name);
    anyhow::ensure!(!dir.exists(), "`{name}` already exists");
    std::fs::create_dir_all(dir)?;

    std::fs::write(
        dir.join("main.claw"),
        "# Welcome to Claw. Run with `claw run`.\n\
         greet = |who| \"Hello, ${who}!\"\n\n\
         main! = |_args| {\n    \
         echo!(greet(\"world\"))\n    \
         Ok({})\n\
         }\n",
    )?;
    std::fs::write(
        dir.join("claw.toml"),
        format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"main.claw\"\n"),
    )?;
    std::fs::write(dir.join(".gitignore"), "/claw.cdb\n/dist\n*.o\n")?;
    std::fs::write(
        dir.join("README.md"),
        format!("# {name}\n\nA Claw project.\n\n```sh\nclaw run\n```\n"),
    )?;

    // Best-effort initial index so the AI guardrail works immediately.
    if let Ok(mut cdb) = Cdb::open(&dir.join("claw.cdb")) {
        let _ = ingest(&mut cdb, &dir.join("main.claw"));
    }

    eprintln!("created project `{name}`");
    eprintln!("  cd {name} && claw run");
    Ok(())
}

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
    eprintln!(
        "indexed {total} definition(s) from {ok}/{} file(s) → {}",
        files.len(),
        db_path.display()
    );
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
    if let Ok(mut dir) = std::env::current_exe() {
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
// Ingest: compiler -> CDB bridge (prototype)
// ---------------------------------------------------------------------
// Wraps the source in a snapshot document, runs the vendored snapshot
// tool (which canonicalizes + typechecks), and zips top-level def names
// (CANONICALIZE section, `(d-let (p-assign (ident "name")))`) with their
// inferred types (TYPES section, `(patt (type "sig"))`) by order.
// Types that our prototype signature parser can't express yet (records,
// tags) fall back to an opaque Named(raw) — still queryable by identity.
// Replaced by a first-class `clawc defs --json` once we patch one in.

/// Extract every quoted payload following `pat` occurrences in `text`.
fn extract_quoted_after(text: &str, pat: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(pat) {
        rest = &rest[i + pat.len()..];
        if let Some(q1) = rest.find('"') {
            let after = &rest[q1 + 1..];
            if let Some(q2) = after.find('"') {
                out.push(after[..q2].to_string());
                rest = &after[q2 + 1..];
            }
        }
    }
    out
}

/// Top-level def names: the first `(ident "...")` after each `(d-let`.
fn extract_def_names(can_ir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = can_ir;
    while let Some(i) = rest.find("(d-let") {
        rest = &rest[i + 6..];
        if let Some(j) = rest.find("(ident \"") {
            let after = &rest[j + 8..];
            if let Some(q) = after.find('"') {
                out.push(after[..q].to_string());
            }
        }
    }
    out
}

fn section<'a>(doc: &'a str, header: &str) -> Option<&'a str> {
    let start = doc.find(header)?;
    let rest = &doc[start + header.len()..];
    let end = rest.find("\n# ").unwrap_or(rest.len());
    Some(&rest[..end])
}

fn ingest(cdb: &mut Cdb, file: &Path) -> anyhow::Result<usize> {
    use claw_core::{Expr, Lit, Type};

    let source = std::fs::read_to_string(file)?;

    // Build a snapshot document around the source and run the tool on it.
    let snap_doc = format!(
        "# META\n~~~ini\ndescription=claw db ingest\ntype=file\n~~~\n# SOURCE\n~~~roc\n{source}~~~\n"
    );
    let tmp = std::env::temp_dir().join(format!("claw-ingest-{}.md", std::process::id()));
    std::fs::write(&tmp, &snap_doc)?;

    let tool = find_tool("snapshot")?;
    let out = std::process::Command::new(&tool).arg(&tmp).output()?;
    anyhow::ensure!(
        out.status.success(),
        "snapshot tool failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The tool rewrites the document in place with generated sections.
    let doc = std::fs::read_to_string(&tmp)?;
    let _ = std::fs::remove_file(&tmp);

    let can = section(&doc, "# CANONICALIZE")
        .ok_or_else(|| anyhow::anyhow!("no CANONICALIZE section in snapshot output"))?;
    let types = section(&doc, "# TYPES")
        .ok_or_else(|| anyhow::anyhow!("no TYPES section in snapshot output"))?;

    let names = extract_def_names(can);
    let sigs = extract_quoted_after(types, "(patt (type ");
    anyhow::ensure!(
        !names.is_empty(),
        "no top-level definitions found in {}",
        file.display()
    );
    anyhow::ensure!(
        names.len() == sigs.len(),
        "defs/types mismatch: {} names vs {} types (destructuring patterns not yet supported)",
        names.len(),
        sigs.len()
    );

    let mut count = 0;
    for (name, sig) in names.iter().zip(&sigs) {
        // Prototype signature parser first; opaque fallback keeps ingest total.
        let ty = parse_type(sig).unwrap_or_else(|_| Type::Named(sig.clone()));
        // Body: opaque source marker for now (real AST lowering comes with
        // the deeper compiler bridge). Unique per (file, name).
        let body = Expr::Lit(Lit::Str(format!("{}::{name}", file.display())));
        let def = Def {
            expr: body,
            ty,
            effects: vec![],
            deprecated: false,
            doc: format!("ingested from {}", file.display()),
        };
        let h = cdb.put(&def)?;
        cdb.bind(name, &h)?;
        count += 1;
    }
    Ok(count)
}
