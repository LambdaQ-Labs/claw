//! claw-mcp — a Model Context Protocol server over the code-as-database.
//!
//! Lets any MCP client (Claude Code, etc.) drive Claw natively: ask what
//! symbols really exist, get the type-directed candidate menu, and get the
//! decode grammar that makes hallucination impossible. This is how an agent
//! writes Claw without inventing APIs — the CDB answers "what's real."
//!
//! Transport: newline-delimited JSON-RPC 2.0 on stdio (MCP stdio).
//! Tools: claw_symbols, claw_candidates, claw_mask.
//!
//! Usage: claw-mcp --db <file>   (default ./claw.cdb)

use claw_cdb::Cdb;
use claw_constraint::{legal_continuations, HoleContext, Mask};
use claw_core::parse::parse_type;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() {
    let db_path = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--db")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "claw.cdb".into());

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(resp) = handle(&req, &db_path) {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
}

fn handle(req: &Value, db_path: &str) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method")?.as_str()?;
    // Notifications (no id) get no response.
    let respond = |result: Value| Some(json!({"jsonrpc":"2.0","id":id,"result":result}));
    let error = |code: i64, msg: &str| {
        Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":msg}}))
    };

    match method {
        "initialize" => respond(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "claw-mcp", "version": env!("CARGO_PKG_VERSION")},
        })),
        "notifications/initialized" => None,
        "tools/list" => respond(json!({"tools": tool_specs()})),
        "tools/call" => {
            let params = req.get("params")?;
            let name = params.get("name")?.as_str()?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            // A TOOL failure is a normal result with isError:true (the
            // agent reads it and recovers) — not a JSON-RPC protocol error.
            match call_tool(name, &args, db_path) {
                Ok(text) => respond(json!({"content": [{"type": "text", "text": text}]})),
                Err(e) => respond(json!({
                    "content": [{"type": "text", "text": e.to_string()}],
                    "isError": true,
                })),
            }
        }
        _ => error(-32601, "method not found"),
    }
}

fn tool_specs() -> Value {
    json!([
        {
            "name": "claw_symbols",
            "description": "List every definition bound in the Claw code-as-database (name : type). The authoritative set of things that exist.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "claw_candidates",
            "description": "Given a target type signature, return the in-scope definitions whose type unifies with it. Use this instead of guessing an API name.",
            "inputSchema": {"type": "object", "properties": {"type": {"type": "string", "description": "type signature, e.g. 'Nat, Nat -> a'"}}, "required": ["type"]}
        },
        {
            "name": "claw_mask",
            "description": "Given a target type, return the legal symbols plus the GBNF grammar that constrains generation so out-of-scope calls are ungeneratable.",
            "inputSchema": {"type": "object", "properties": {"type": {"type": "string"}}, "required": ["type"]}
        }
    ])
}

fn call_tool(name: &str, args: &Value, db_path: &str) -> anyhow::Result<String> {
    let cdb = Cdb::open(std::path::Path::new(db_path))?;
    match name {
        "claw_symbols" => {
            let mut out = String::new();
            for (n, h) in cdb.symbols()? {
                let d = cdb.get(&h)?;
                out.push_str(&format!("{n} : {}\n", d.ty));
            }
            Ok(if out.is_empty() {
                "(no symbols)".into()
            } else {
                out
            })
        }
        "claw_candidates" => {
            let ty = parse_type(args.get("type").and_then(|v| v.as_str()).unwrap_or(""))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut out = String::new();
            for c in cdb.candidates(&ty)? {
                out.push_str(&format!(
                    "{} : {}{}\n",
                    c.name,
                    c.ty,
                    if c.deprecated { "  [deprecated]" } else { "" }
                ));
            }
            Ok(if out.is_empty() {
                "(no candidates)".into()
            } else {
                out
            })
        }
        "claw_mask" => {
            let ty = parse_type(args.get("type").and_then(|v| v.as_str()).unwrap_or(""))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let hole = HoleContext {
                editing: None,
                expected: ty,
            };
            match legal_continuations(&cdb, &hole)? {
                Mask::Symbols(list) => {
                    let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
                    Ok(format!(
                        "legal symbols: {}\n\n--- GBNF ---\n{}",
                        names.join(", "),
                        claw_constraint::gbnf::def_json_grammar(&list)
                    ))
                }
                Mask::EmptyWithDiagnostic(d) => Ok(format!("no legal symbols: {}", d.render())),
            }
        }
        other => anyhow::bail!("unknown tool: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_protocol_and_tools() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
        let resp = handle(&req, "unused.cdb").unwrap();
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_has_the_three_tools() {
        let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let resp = handle(&req, "unused.cdb").unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"claw_symbols"));
        assert!(names.contains(&"claw_candidates"));
        assert!(names.contains(&"claw_mask"));
    }

    #[test]
    fn notification_gets_no_response() {
        let req = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle(&req, "unused.cdb").is_none());
    }

    #[test]
    fn unknown_method_errors() {
        let req = json!({"jsonrpc":"2.0","id":3,"method":"nope"});
        let resp = handle(&req, "unused.cdb").unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }
}
