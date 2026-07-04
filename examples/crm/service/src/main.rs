//! Claw CRM — the service layer (Postgres + HTTP).
//!
//! Claw can't open a database connection or route HTTP yet (no such host),
//! so this thin Rust service provides the I/O. The *business rules* — the
//! pipeline state machine — are authored and machine-verified in Claw
//! (`../domain.claw`, run with `claw db eval` / `claw db check`); this
//! service enforces the exact same transitions on real data.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::collections::HashMap;

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn err(code: StatusCode, msg: impl ToString) -> (StatusCode, String) {
    (code, msg.to_string())
}

/// The pipeline state machine — the mirror of `advance` in domain.claw.
/// (Lead → Qualified → Proposal → Negotiation → Won; terminals stay.)
fn advance_stage(stage: &str) -> &str {
    match stage {
        "Lead" => "Qualified",
        "Qualified" => "Proposal",
        "Proposal" => "Negotiation",
        "Negotiation" => "Won",
        other => other, // Won / Lost are terminal
    }
}

const STAGES: &[&str] = &[
    "Lead",
    "Qualified",
    "Proposal",
    "Negotiation",
    "Won",
    "Lost",
];

#[tokio::main]
async fn main() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://ninad@localhost:5432/claw_crm".into());
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("connect to Postgres");
    init_schema(&pool).await.expect("init schema");

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/customers", post(create_customer).get(list_customers))
        .route("/customers/:id", get(get_customer))
        .route("/deals", post(create_deal).get(list_deals))
        .route("/deals/:id", get(get_deal))
        .route("/deals/:id/advance", post(advance_deal))
        .route("/deals/:id/lose", post(lose_deal))
        .route("/pipeline", get(pipeline))
        .with_state(pool);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8787")
        .await
        .expect("bind :8787");
    eprintln!("claw-crm listening on http://127.0.0.1:8787");
    axum::serve(listener, app).await.unwrap();
}

async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS customers (
            id SERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            email TEXT NOT NULL UNIQUE
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS deals (
            id SERIAL PRIMARY KEY,
            customer_id INTEGER NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            value_cents BIGINT NOT NULL DEFAULT 0,
            stage TEXT NOT NULL DEFAULT 'Lead',
            updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

// --- customers ---------------------------------------------------------

#[derive(Deserialize)]
struct NewCustomer {
    name: String,
    email: String,
}

async fn create_customer(State(pool): State<PgPool>, Json(c): Json<NewCustomer>) -> ApiResult {
    let row = sqlx::query(
        "INSERT INTO customers (name, email) VALUES ($1, $2) RETURNING id, name, email",
    )
    .bind(&c.name)
    .bind(&c.email)
    .fetch_one(&pool)
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({
        "id": row.get::<i32, _>("id"),
        "name": row.get::<String, _>("name"),
        "email": row.get::<String, _>("email"),
    })))
}

async fn list_customers(State(pool): State<PgPool>) -> ApiResult {
    let rows = sqlx::query("SELECT id, name, email FROM customers ORDER BY id")
        .fetch_all(&pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<i32, _>("id"),
                "name": r.get::<String, _>("name"),
                "email": r.get::<String, _>("email"),
            })
        })
        .collect();
    Ok(Json(json!(out)))
}

async fn get_customer(State(pool): State<PgPool>, Path(id): Path<i32>) -> ApiResult {
    let row = sqlx::query("SELECT id, name, email FROM customers WHERE id = $1")
        .bind(id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such customer"))?;
    Ok(Json(json!({
        "id": row.get::<i32, _>("id"),
        "name": row.get::<String, _>("name"),
        "email": row.get::<String, _>("email"),
    })))
}

// --- deals -------------------------------------------------------------

#[derive(Deserialize)]
struct NewDeal {
    customer_id: i32,
    title: String,
    #[serde(default)]
    value_cents: i64,
}

fn deal_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({
        "id": r.get::<i32, _>("id"),
        "customer_id": r.get::<i32, _>("customer_id"),
        "title": r.get::<String, _>("title"),
        "value_cents": r.get::<i64, _>("value_cents"),
        "stage": r.get::<String, _>("stage"),
    })
}

async fn create_deal(State(pool): State<PgPool>, Json(d): Json<NewDeal>) -> ApiResult {
    let row = sqlx::query(
        "INSERT INTO deals (customer_id, title, value_cents, stage)
         VALUES ($1, $2, $3, 'Lead')
         RETURNING id, customer_id, title, value_cents, stage",
    )
    .bind(d.customer_id)
    .bind(&d.title)
    .bind(d.value_cents)
    .fetch_one(&pool)
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(deal_json(&row)))
}

async fn list_deals(State(pool): State<PgPool>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let rows = if let Some(stage) = q.get("stage") {
        sqlx::query(
            "SELECT id, customer_id, title, value_cents, stage FROM deals WHERE stage = $1 ORDER BY id",
        )
        .bind(stage)
        .fetch_all(&pool)
        .await
    } else {
        sqlx::query("SELECT id, customer_id, title, value_cents, stage FROM deals ORDER BY id")
            .fetch_all(&pool)
            .await
    }
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!(rows.iter().map(deal_json).collect::<Vec<_>>())))
}

async fn get_deal(State(pool): State<PgPool>, Path(id): Path<i32>) -> ApiResult {
    let row = sqlx::query("SELECT id, customer_id, title, value_cents, stage FROM deals WHERE id = $1")
        .bind(id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such deal"))?;
    Ok(Json(deal_json(&row)))
}

/// Move a deal one step forward — enforcing the Claw pipeline rules.
async fn advance_deal(State(pool): State<PgPool>, Path(id): Path<i32>) -> ApiResult {
    let cur = sqlx::query("SELECT stage FROM deals WHERE id = $1")
        .bind(id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such deal"))?;
    let stage: String = cur.get("stage");
    let next = advance_stage(&stage);
    let row = sqlx::query(
        "UPDATE deals SET stage = $1, updated_at = now() WHERE id = $2
         RETURNING id, customer_id, title, value_cents, stage",
    )
    .bind(next)
    .bind(id)
    .fetch_one(&pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "from": stage, "to": next, "deal": deal_json(&row) })))
}

async fn lose_deal(State(pool): State<PgPool>, Path(id): Path<i32>) -> ApiResult {
    let row = sqlx::query(
        "UPDATE deals SET stage = 'Lost', updated_at = now() WHERE id = $1
         RETURNING id, customer_id, title, value_cents, stage",
    )
    .bind(id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such deal"))?;
    Ok(Json(deal_json(&row)))
}

/// Pipeline summary: count + total value per stage.
async fn pipeline(State(pool): State<PgPool>) -> ApiResult {
    let rows = sqlx::query(
        "SELECT stage, COUNT(*)::BIGINT AS n, COALESCE(SUM(value_cents),0)::BIGINT AS total
         FROM deals GROUP BY stage",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let mut by_stage: HashMap<String, Value> = rows
        .iter()
        .map(|r| {
            (
                r.get::<String, _>("stage"),
                json!({ "count": r.get::<i64, _>("n"), "total_value_cents": r.get::<i64, _>("total") }),
            )
        })
        .collect();
    // present every stage, even empty ones, in pipeline order
    let out: Vec<Value> = STAGES
        .iter()
        .map(|s| {
            let v = by_stage
                .remove(*s)
                .unwrap_or_else(|| json!({ "count": 0, "total_value_cents": 0 }));
            json!({ "stage": s, "count": v["count"], "total_value_cents": v["total_value_cents"] })
        })
        .collect();
    Ok(Json(json!(out)))
}
