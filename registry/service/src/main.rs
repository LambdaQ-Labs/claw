//! claw-registry — a package registry for Claw (npmjs.com-style).
//!
//! Claw packages are content-addressed `.tar.zst` bundles (`claw bundle`
//! names them by hash). This registry stores them by name+version, serves
//! the raw bundle at a stable URL the Claw compiler can fetch (localhost
//! downloads are allowed), and exposes metadata + a simple index.
//!
//! Storage: bundle blobs on disk (registry_data/blobs/<hash>.tar.zst),
//! the name→version→hash index in Postgres.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::path::PathBuf;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    blobs: PathBuf,
    base_url: String,
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;
fn err(c: StatusCode, m: impl ToString) -> (StatusCode, String) {
    (c, m.to_string())
}

#[tokio::main]
async fn main() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://ninad@localhost:5432/claw_registry".into());
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8888);
    let base_url = std::env::var("CLAW_REGISTRY_URL")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
    let blobs = PathBuf::from(
        std::env::var("CLAW_REGISTRY_DATA").unwrap_or_else(|_| "registry_data/blobs".into()),
    );
    std::fs::create_dir_all(&blobs).expect("create blob dir");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("connect Postgres");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS packages (
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            hash TEXT NOT NULL,
            filename TEXT NOT NULL,
            size BIGINT NOT NULL,
            published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            PRIMARY KEY (name, version)
        )",
    )
    .execute(&pool)
    .await
    .expect("schema");

    let state = AppState { pool, blobs, base_url };
    let app = Router::new()
        .route("/", get(index))
        .route("/publish", post(publish))
        .route("/packages/:name", get(package_meta))
        .route("/b/:filename", get(serve_blob)) // the compiler fetches this
        .route("/defs/:name/:version", get(serve_defs)) // the AI layer fetches this
        .with_state(state)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024));

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("claw-registry on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}

/// `POST /publish` — multipart: fields `name`, `version`, and file `bundle`
/// (a `.tar.zst` whose base filename is its content hash). Stores the blob
/// + index row. Returns the URL to reference it by.
async fn publish(State(st): State<AppState>, mut mp: Multipart) -> ApiResult {
    let (mut name, mut version, mut filename, mut bytes) =
        (None, None, None, None::<Vec<u8>>);
    let mut defs_bytes = None::<Vec<u8>>;
    while let Some(field) = mp.next_field().await.map_err(|e| err(StatusCode::BAD_REQUEST, e))? {
        match field.name().unwrap_or("") {
            "name" => name = Some(field.text().await.map_err(|e| err(StatusCode::BAD_REQUEST, e))?),
            "version" => {
                version = Some(field.text().await.map_err(|e| err(StatusCode::BAD_REQUEST, e))?)
            }
            "bundle" => {
                filename = field.file_name().map(|s| s.to_string());
                bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?
                        .to_vec(),
                );
            }
            "defs" => {
                defs_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }
    let name = name.ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing name"))?;
    let version = version.ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing version"))?;
    let filename = filename.ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing bundle"))?;
    let bytes = bytes.ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing bundle bytes"))?;
    // MCP-compatibility gate: every package MUST carry its definitions
    // (name : type [+ effects, doc]) so a consumer's code database — and
    // therefore any AI wired to it — understands the package on install.
    let defs_bytes = defs_bytes.ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "missing defs — packages must publish their definitions \
             (claw publish generates these; update your claw CLI)",
        )
    })?;
    let defs: Vec<serde_json::Value> = serde_json::from_slice(&defs_bytes)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("defs is not JSON: {e}")))?;
    if defs.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "defs is empty — a package must expose at least one definition"));
    }
    for d in &defs {
        let n = d["name"].as_str().unwrap_or("");
        let t = d["ty"].as_str().unwrap_or("");
        if n.is_empty() {
            return Err(err(StatusCode::BAD_REQUEST, "a def is missing its name"));
        }
        claw_core::parse::parse_type(t).map_err(|e| {
            err(StatusCode::BAD_REQUEST, format!("def `{n}` has an unparseable type `{t}`: {e}"))
        })?;
    }

    // Hash = the bundle's base filename (content-addressed by `claw bundle`).
    let hash = filename.trim_end_matches(".tar.zst").to_string();

    std::fs::write(st.blobs.join(&filename), &bytes)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    std::fs::write(st.blobs.join(format!("{name}-{version}.defs.json")), &defs_bytes)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    sqlx::query(
        "INSERT INTO packages (name, version, hash, filename, size)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (name, version) DO UPDATE
           SET hash=excluded.hash, filename=excluded.filename, size=excluded.size",
    )
    .bind(&name)
    .bind(&version)
    .bind(&hash)
    .bind(&filename)
    .bind(bytes.len() as i64)
    .execute(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let url = format!("{}/b/{}", st.base_url, filename);
    Ok(Json(json!({ "name": name, "version": version, "hash": hash, "url": url })))
}

/// `GET /packages/:name` — versions + the URL to fetch each.
async fn package_meta(State(st): State<AppState>, Path(name): Path<String>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT version, hash, filename, size FROM packages WHERE name=$1 ORDER BY version",
    )
    .bind(&name)
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if rows.is_empty() {
        return Err(err(StatusCode::NOT_FOUND, "no such package"));
    }
    let versions: Vec<Value> = rows
        .iter()
        .map(|r| {
            let filename: String = r.get("filename");
            json!({
                "version": r.get::<String,_>("version"),
                "hash": r.get::<String,_>("hash"),
                "size": r.get::<i64,_>("size"),
                "url": format!("{}/b/{}", st.base_url, filename),
            })
        })
        .collect();
    let latest = &versions[versions.len() - 1];
    Ok(Json(json!({ "name": name, "latest": latest, "versions": versions })))
}

/// `GET /b/:filename` — the raw bundle. This is the URL the Claw compiler
/// downloads (and verifies against the hash in the filename).
/// `GET /defs/:name/:version` — the package's definitions for the
/// consumer's code database (the MCP-compatibility payload).
async fn serve_defs(
    State(st): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    match std::fs::read(st.blobs.join(format!("{name}-{version}.defs.json"))) {
        Ok(b) => ([("content-type", "application/json")], b).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_blob(State(st): State<AppState>, Path(filename): Path<String>) -> impl IntoResponse {
    // No path traversal.
    if filename.contains('/') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad filename").into_response();
    }
    match std::fs::read(st.blobs.join(&filename)) {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, "application/zstd")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no such bundle").into_response(),
    }
}

/// `GET /` — a minimal registry index (npmjs-style listing).
async fn index(State(st): State<AppState>) -> Html<String> {
    let rows = sqlx::query(
        "SELECT DISTINCT ON (name) name, version, size FROM packages ORDER BY name, version DESC",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let mut list = String::new();
    for r in &rows {
        let n: String = r.get("name");
        let v: String = r.get("version");
        let s: i64 = r.get("size");
        list.push_str(&format!(
            "<li><code>{n}</code> <span>{v}</span> <em>{s} bytes</em> — <code>claw add {n}</code></li>"
        ));
    }
    if list.is_empty() {
        list = "<li><em>no packages yet — `claw publish` one</em></li>".into();
    }
    Html(format!(
        "<!doctype html><meta charset=utf8><title>Claw registry</title>\
         <style>body{{font:16px system-ui;max-width:720px;margin:3rem auto}}\
         li{{margin:.4rem 0}}span{{color:#888}}em{{color:#aaa;font-size:.85em}}</style>\
         <h1>🐾 Claw registry</h1><p>{} package(s). Publish with <code>claw publish</code>, \
         install with <code>claw add &lt;name&gt;</code>.</p><ul>{}</ul>",
        rows.len(),
        list
    ))
}
