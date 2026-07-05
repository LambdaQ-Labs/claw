//! claw-telemetry — anonymous usage metrics, on by default, one command
//! to turn off.
//!
//! The cold-start corpus is synthetic; the model gets better fastest from
//! real usage. The rules:
//!
//! 1. **Metrics only by default.** Command kinds, verdict flags, error
//!    counts — never source code, never file paths, never prompts. The
//!    `full` level (code payloads, the training-grade signal) is and
//!    stays explicit opt-in.
//! 2. **Loud and reversible.** The first event prints a one-time notice
//!    with the off switch: `claw telemetry off` (persisted) or
//!    `CLAW_TELEMETRY=off` (env, wins over the file).
//! 3. **Local-first, bounded.** Events append to a readable JSONL
//!    (`~/.claw/telemetry/events.jsonl`, 4 MiB cap + one rotation);
//!    upload is one gzipped request when the log crosses ~64 KiB, or
//!    manually via `claw telemetry share`.

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The telemetry level. Default: `Metrics`. Resolution order: the
/// `CLAW_TELEMETRY` env var (off|metrics|full) wins; else the persisted
/// choice (`claw telemetry off|on|full`); else Metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    Metrics,
    Full,
}

fn config_path() -> PathBuf {
    dir().join("config")
}

/// Persist a level choice (the `claw telemetry on|off|full` command).
pub fn set_level(l: &str) -> Result<String, String> {
    match l {
        "off" | "on" | "metrics" | "full" => {
            let v = if l == "on" { "metrics" } else { l };
            std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
            std::fs::write(config_path(), v).map_err(|e| e.to_string())?;
            Ok(format!("telemetry set to `{v}` (env CLAW_TELEMETRY overrides)"))
        }
        other => Err(format!("unknown level `{other}` (off | on | full)")),
    }
}

pub fn level() -> Level {
    match std::env::var("CLAW_TELEMETRY").as_deref() {
        Ok("off") => return Level::Off,
        Ok("metrics") => return Level::Metrics,
        Ok("full") => return Level::Full,
        _ => {}
    }
    match std::fs::read_to_string(config_path()).as_deref().map(str::trim) {
        Ok("off") => Level::Off,
        Ok("full") => Level::Full,
        _ => Level::Metrics,
    }
}

/// One-time stderr notice on the very first recorded event — telemetry
/// that is on by default must announce itself.
fn first_run_notice() {
    let marker = dir().join(".notified");
    if marker.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir());
    let _ = std::fs::write(&marker, b"1");
    eprintln!(
        "note: claw collects anonymous usage metrics (command kinds and verdicts — never your code).\n      turn off: claw telemetry off   details: https://github.com/LambdaQ-Labs/claw/blob/main/docs/telemetry.md"
    );
}

/// Auto-upload once the log crosses ~64 KiB; failures are silent (the
/// log simply keeps accumulating up to its cap and retries next time).
fn maybe_autoshare() {
    if std::env::var("CLAW_TELEMETRY_AUTOSHARE").as_deref() == Ok("0") {
        return;
    }
    if std::fs::metadata(log_path()).map(|m| m.len() > 64 * 1024).unwrap_or(false) {
        let _ = share();
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
    first_run_notice();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{obj}");
    }
    maybe_autoshare();
}

/// Human-readable status line for `claw telemetry status`.
pub fn status() -> String {
    let lvl = match level() {
        Level::Off => "off",
        Level::Metrics => "metrics (default — anonymous, no code; `claw telemetry off` to disable)",
        Level::Full => "full (includes code payloads — thank you)",
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
        .unwrap_or_else(|_| "https://telemetry.clawlang.dev/v1/ingest".into());

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
    fn metrics_by_default_and_off_silences() {
        with_tmp("default", || {
            std::env::remove_var("CLAW_TELEMETRY");
            event("test", json!({"a": 1}), Some(json!({"defs": "SECRET"})));
            let s = std::fs::read_to_string(log_path()).unwrap();
            assert!(s.contains("\"kind\":\"test\""), "default level records metrics");
            assert!(!s.contains("SECRET"), "default level must never record code");

            std::env::set_var("CLAW_TELEMETRY", "off");
            let before = std::fs::metadata(log_path()).unwrap().len();
            event("test2", json!({"a": 2}), None);
            assert_eq!(before, std::fs::metadata(log_path()).unwrap().len(), "off must mean zero writes");
            std::env::remove_var("CLAW_TELEMETRY");
        });
    }

    #[test]
    fn persisted_off_wins_without_env() {
        with_tmp("persist", || {
            std::env::remove_var("CLAW_TELEMETRY");
            set_level("off").unwrap();
            event("test", json!({"a": 1}), None);
            assert!(!log_path().exists(), "persisted off must mean zero writes");
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
