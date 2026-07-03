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
        // Compiler passthrough: `claw check|build|fmt|test|repl <args>` runs
        // the vendored compiler (clawc). CLAW_COMPILER overrides discovery.
        Some(cmd @ ("check" | "build" | "fmt" | "test" | "repl")) => {
            let status = std::process::Command::new(find_clawc()?)
                .arg(cmd)
                .args(&args[1..])
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        _ => {
            eprintln!(
                "claw — the Claw toolchain\n\nusage:\n  claw check|build|fmt|test|repl <file.claw>   (compiler)\n  claw [--db <file>] db <subcommand>           (code-as-database)\n\ndb subcommands:\n  symbols | put | bind <name> <hash> | resolve <name> | ingest <file.claw>\n  candidates \"<type>\" | callers <ref> | deps <ref> | render <ref> | mask \"<type>\""
            );
            std::process::exit(2);
        }
    }
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
            let h = resolve_ref(&cdb, need(args, 1, "name|hash")?)?;
            println!("{}", serde_json::to_string_pretty(&cdb.get(&h)?)?);
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
