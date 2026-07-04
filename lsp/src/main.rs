//! claw-lsp — a minimal Language Server over the code-as-database.
//!
//! Editor support backed by the CDB: completion offers the real in-scope
//! symbols (so an author can only pick things that exist, mirroring the
//! agent's constrained generation), and hover shows a symbol's type. This
//! is intentionally small — the point is that the same "what's real" source
//! the compiler and the model use also powers the editor.
//!
//! Transport: LSP framing (Content-Length headers + JSON-RPC). Requests
//! handled: initialize, textDocument/completion, textDocument/hover,
//! shutdown. Store path via --db (default ./claw.cdb).

use claw_cdb::Cdb;
use serde_json::{json, Value};
use std::io::{Read, Write};

fn main() {
    let db_path = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--db")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "claw.cdb".into());

    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    while let Some(msg) = read_message(&mut stdin) {
        if let Some(resp) = handle(&msg, &db_path) {
            write_message(&mut stdout, &resp);
        }
        if msg.get("method").and_then(|m| m.as_str()) == Some("exit") {
            break;
        }
    }
}

/// Read one LSP message: `Content-Length: N\r\n\r\n<N bytes of JSON>`.
fn read_message(r: &mut impl Read) -> Option<Value> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    // read until the blank line terminating the headers
    while !header.ends_with(b"\r\n\r\n") {
        if r.read_exact(&mut byte).is_err() {
            return None;
        }
        header.push(byte[0]);
    }
    let header = String::from_utf8_lossy(&header);
    let len: usize = header
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))
        .and_then(|v| v.trim().parse().ok())?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message(w: &mut impl Write, v: &Value) {
    let body = v.to_string();
    let _ = write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = w.flush();
}

fn handle(req: &Value, db_path: &str) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method")?.as_str()?;
    let respond = |result: Value| Some(json!({"jsonrpc":"2.0","id":id,"result":result}));

    match method {
        "initialize" => respond(json!({
            "capabilities": {
                "completionProvider": { "triggerCharacters": ["."] },
                "hoverProvider": true,
                "textDocumentSync": 1
            },
            "serverInfo": { "name": "claw-lsp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "initialized" => None,
        "textDocument/completion" => respond(completions(db_path)),
        "textDocument/hover" => respond(hover(req, db_path)),
        "shutdown" => respond(Value::Null),
        _ => {
            // Unknown request with an id → empty result; notifications → none.
            id.as_ref()?;
            respond(Value::Null)
        }
    }
}

/// Every bound CDB symbol as a completion item labelled with its type.
fn completions(db_path: &str) -> Value {
    let items: Vec<Value> = match Cdb::open(std::path::Path::new(db_path)) {
        Ok(cdb) => cdb
            .symbols()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(n, h)| {
                let d = cdb.get(&h).ok()?;
                Some(json!({
                    "label": n,
                    "kind": 3, // Function
                    "detail": d.ty.to_string(),
                    "documentation": if d.deprecated { "deprecated" } else { "" },
                }))
            })
            .collect(),
        Err(_) => vec![],
    };
    json!({ "isIncomplete": false, "items": items })
}

/// Hover: if the request carries a symbol name in params, show its type.
fn hover(req: &Value, db_path: &str) -> Value {
    let name = req
        .get("params")
        .and_then(|p| p.get("symbol"))
        .and_then(|s| s.as_str());
    if let (Some(name), Ok(cdb)) = (name, Cdb::open(std::path::Path::new(db_path))) {
        if let Ok(h) = cdb.resolve(name) {
            if let Ok(d) = cdb.get(&h) {
                return json!({ "contents": { "kind": "plaintext", "value": format!("{name} : {}", d.ty) } });
            }
        }
    }
    Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_roundtrip() {
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
        let mut buf = Vec::new();
        write_message(&mut buf, &msg);
        let mut cur = Cursor::new(buf);
        let back = read_message(&mut cur).unwrap();
        assert_eq!(back["method"], "initialize");
    }

    #[test]
    fn initialize_advertises_completion_and_hover() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
        let resp = handle(&req, "unused.cdb").unwrap();
        assert!(resp["result"]["capabilities"]["hoverProvider"]
            .as_bool()
            .unwrap());
        assert!(resp["result"]["capabilities"]["completionProvider"].is_object());
    }

    #[test]
    fn completion_on_empty_db_is_empty_list_not_error() {
        let c = completions("nonexistent.cdb");
        assert_eq!(c["items"].as_array().unwrap().len(), 0);
        assert_eq!(c["isIncomplete"], false);
    }

    #[test]
    fn shutdown_responds_null() {
        let req = json!({"jsonrpc":"2.0","id":9,"method":"shutdown"});
        let resp = handle(&req, "unused.cdb").unwrap();
        assert!(resp["result"].is_null());
    }
}
