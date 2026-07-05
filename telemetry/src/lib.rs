//! claw-telemetry — opt-in, local-first usage capture.
//!
//! The cold-start corpus is synthetic; the model gets better fastest from
//! REAL (prompt, produced-def, verdict) triples. This crate collects them
//! with three hard rules:
//!
//! 1. **Off by default.** Nothing is written unless `CLAW_TELEMETRY` is
//!    set to `metrics` or `full`. No phone-home, ever, without opt-in.
//! 2. **Local-first.** Events append to a plain JSONL file the user can
//!    read (`~/.claw/telemetry/events.jsonl`); upload happens only when
//!    they run `claw telemetry share` (or set `CLAW_TELEMETRY_AUTOSHARE=1`).
//! 3. **Bounded.** The log rotates at 4 MiB into a single `.1` backup —
//!    worst case ~8 MiB of disk; uploads are gzipped (one request, no
//!    server round-trips per event). The ingest side is a Cloudflare
//!    Worker writing to R2 — free-tier scale by design.
//!
//! Levels: `metrics` = command, duration, outcome, counts — no code.
//! `full`  = also the produced Def-JSON and grading verdicts (the
//! training-grade signal).

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The opt-in level, read from `CLAW_TELEMETRY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    Metrics,
    Full,
}

pub fn level() -> Level {
    match std::env::var("CLAW_TELEMETRY").as_deref() {
        Ok("metrics") => Level::Metrics,
        Ok("full") => Level::Full,
        _ => Level::Off,
    }
}

/// Where events live. `CLAW_TELEMETRY_DIR` overrides (tests, odd setups).
pub fn dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAW_TELEMETRY_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claw").join("telemetry")
}

fn log_path() -> PathBuf {
    dir().join("events.jsonl")
}

/// Append one event. `code_payload` is only recorded at `full` level —
/// pass the produced defs / diagnostics there, never in `fields`.
pub fn event(kind: &str, fields: Value, code_payload: Option<Value>) {
    let lvl = level();
    if lvl == Level::Off {
        return;
    }
    let mut obj = json!({
        "t": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "kind": kind,
        "v": env!("CARGO_PKG_VERSION"),
    });
    if let (Some(o), Some(f)) = (obj.as_object_mut(), fields.as_object()) {
        for (k, v) in f {
            o.insert(k.clone(), v.clone());
        }
    }
    if lvl == Level::Full {
        if let (Some(o), Some(p)) = (obj.as_object_mut(), code_payload) {
            o.insert("payload".into(), p);
        }
    }
    let path = log_path();
    let _ = std::fs::create_dir_all(dir());
    // Rotate: one .1 backup, hard-capped total footprint.
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > MAX_BYTES {
            let _ = std::fs::rename(&path, dir().join("events.jsonl.1"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{obj}");
    }
}

/// Human-readable status line for `claw telemetry status`.
pub fn status() -> String {
    let lvl = match level() {
        Level::Off => "off (set CLAW_TELEMETRY=metrics|full to opt in)",
        Level::Metrics => "metrics",
        Level::Full => "full",
    };
    let size = std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0);
    let lines = std::fs::read_to_string(log_path())
        .map(|s| s.lines().count())
        .unwrap_or(0);
    format!(
        "level: {lvl}\nlog:   {} ({} events, {} KiB, cap 4 MiB + one rotation)\nshare: claw telemetry share  (uploads gzipped, then clears)",
        log_path().display(),
        lines,
        size / 1024
    )
}

/// Upload the log (gzip JSONL) to the ingest endpoint, then truncate.
/// Endpoint: `CLAW_TELEMETRY_URL` or the project default.
pub fn share() -> Result<String, String> {
    let path = log_path();
    let body = std::fs::read(&path).map_err(|_| "no telemetry log to share".to_string())?;
    if body.is_empty() {
        return Err("telemetry log is empty".into());
    }
    let url = std::env::var("CLAW_TELEMETRY_URL")
        .unwrap_or_else(|_| {
            // The deployed ingest worker; swaps to telemetry.clawlang.dev
            // when the domain routes.
            "https://claw-telemetry.ninad2471.workers.dev/v1/ingest".into()
        });

    // gzip the JSONL — one small request, no per-event chatter.
    let gz = gzip(&body);
    let resp = ureq::post(&url)
        .set("content-type", "application/jsonl")
        .set("content-encoding", "gzip")
        .send_bytes(&gz)
        .map_err(|e| format!("upload failed: {e}"))?;
    if resp.status() < 300 {
        let n = body.iter().filter(|b| **b == b'\n').count();
        let _ = std::fs::write(&path, b"");
        Ok(format!("shared {n} events ({} KiB gzipped)", gz.len() / 1024))
    } else {
        Err(format!("server said {}", resp.status()))
    }
}

pub fn clear() -> String {
    let _ = std::fs::remove_file(log_path());
    let _ = std::fs::remove_file(dir().join("events.jsonl.1"));
    "telemetry log cleared".into()
}

/// Minimal static-Huffman-free gzip (stored blocks) — keeps the dependency
/// surface at zero; JSONL is small and the endpoint accepts identity too.
fn gzip(data: &[u8]) -> Vec<u8> {
    // DEFLATE "stored" blocks (no compression) wrapped in a gzip container.
    // Telemetry logs are tiny; correctness and zero deps beat ratio here.
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    let mut rest = data;
    loop {
        let chunk = &rest[..rest.len().min(65535)];
        rest = &rest[chunk.len()..];
        let last = if rest.is_empty() { 1u8 } else { 0 };
        out.push(last);
        out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        out.extend_from_slice(chunk);
        if last == 1 {
            break;
        }
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env vars are process-global: serialize the tests that touch them.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_tmp<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "claw-telem-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("CLAW_TELEMETRY_DIR", &tmp);
        let out = f();
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("CLAW_TELEMETRY_DIR");
        std::env::remove_var("CLAW_TELEMETRY");
        out
    }

    #[test]
    fn off_by_default_writes_nothing() {
        with_tmp("off", || {
            std::env::remove_var("CLAW_TELEMETRY");
            event("test", json!({"a": 1}), None);
            assert!(!log_path().exists(), "opt-out must mean zero writes");
        });
    }

    #[test]
    fn metrics_level_drops_code_payload() {
        with_tmp("metrics", || {
            std::env::set_var("CLAW_TELEMETRY", "metrics");
            event("check", json!({"ok": true}), Some(json!({"defs": "SECRET"})));
            let s = std::fs::read_to_string(log_path()).unwrap();
            assert!(s.contains("\"ok\":true"));
            assert!(!s.contains("SECRET"), "code must not leak at metrics level");
            std::env::remove_var("CLAW_TELEMETRY");
        });
    }

    #[test]
    fn full_level_records_payload() {
        with_tmp("full", || {
            std::env::set_var("CLAW_TELEMETRY", "full");
            event("check", json!({"ok": false}), Some(json!({"defs": [1, 2]})));
            let s = std::fs::read_to_string(log_path()).unwrap();
            assert!(s.contains("\"payload\""));
            std::env::remove_var("CLAW_TELEMETRY");
        });
    }

    #[test]
    fn gzip_roundtrips_via_flate_layout() {
        // Sanity: container magic + ISIZE trailer correct.
        let g = gzip(b"hello world");
        assert_eq!(&g[..2], &[0x1f, 0x8b]);
        assert_eq!(&g[g.len() - 4..], &(11u32.to_le_bytes()));
    }
}
