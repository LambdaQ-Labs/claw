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
use std::path::PathBuf;

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
        _ => {
            eprintln!(
                "claw — the Claw toolchain\n\nusage:\n  claw [--db <file>] db <subcommand>\n\nsubcommands:\n  symbols | put | bind <name> <hash> | resolve <name>\n  candidates \"<type>\" | callers <ref> | deps <ref> | render <ref> | mask \"<type>\""
            );
            std::process::exit(2);
        }
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

fn db_cmd(db_path: &PathBuf, args: &[String]) -> anyhow::Result<()> {
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
