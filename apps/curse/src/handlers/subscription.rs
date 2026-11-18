//! Subscription endpoint: `GET /sub/{token}.{fmt}`.
//!
//! M1b pipeline: hash-verified token -> profile -> linked nodes joined with score,
//! region, health and the **sealed** payload -> decrypt -> filter AST / region /
//! min-score / preset / quota / naming / sponsor via
//! `distribution::subscription::render_subscription_with_sponsor`.
//!
//! `base64` is **not** served yet: the pure pipeline's base64 branch still fabricates
//! links (`{proto}://cred@{id}:443`), and a real Clash->URI builder lands with M1c.
//! Returning 501 is deliberate — serving fake links would be worse than refusing.
use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use distribution::subscription::{
    render_subscription_with_sponsor, PipelineNode, ProfileSpec, RenderedSubscription,
};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

/// The profile columns the renderer needs.
struct ProfileRow {
    filter_ast: String,
    preset: String,
    region: Option<String>,
    health: String,
    min_score: f64,
    limit_count: i32,
    naming_rule: Option<String>,
}

/// token hash -> profile, only for active tokens on enabled profiles.
type ProfileTuple = (
    Option<String>,
    String,
    Option<String>,
    String,
    f64,
    i32,
    Option<String>,
);

async fn lookup_profile(pool: &PgPool, token_hash: &str) -> Option<ProfileRow> {
    let row: Option<ProfileTuple> = sqlx::query_as(
        "SELECT p.filter_ast::text, p.preset, p.region, p.health, p.min_score, \
                    p.limit_count, p.naming_rule \
             FROM subscription_tokens t JOIN subscription_profiles p ON p.id = t.profile_id \
             WHERE t.token_hash=$1 AND t.status='active' AND p.enabled",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(
        |(filter_ast, preset, region, health, min_score, limit_count, naming_rule)| ProfileRow {
            filter_ast: filter_ast.unwrap_or_default(),
            preset,
            region,
            health,
            min_score,
            limit_count,
            naming_rule,
        },
    )
}

/// One linked node with everything the renderer and the sponsor matcher need.
struct NodeRow {
    id: Uuid,
    protocol: String,
    server: String,
    port: i32,
    score: f64,
    region: String,
    health: String,
    nonce: Option<Vec<u8>>,
    ciphertext: Option<Vec<u8>>,
}

async fn load_nodes(pool: &PgPool) -> Result<Vec<NodeRow>, sqlx::Error> {
    type Row = (
        Uuid,
        String,
        String,
        i32,
        f64,
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT n.id, n.protocol, n.server, n.port, \
                COALESCE(sc.score,0)::float8, \
                reg.region, st.health, np.nonce, np.ciphertext \
         FROM nodes n \
         LEFT JOIN (SELECT node_id, max(score) AS score FROM node_scores GROUP BY node_id) sc \
                ON sc.node_id = n.id \
         LEFT JOIN LATERAL (SELECT r.metrics->>'region' AS region FROM node_check_results r \
                            WHERE r.node_id = n.id AND r.metrics ? 'region' \
                            ORDER BY r.checked_at DESC LIMIT 1) reg ON true \
         LEFT JOIN (SELECT node_id, \
                           CASE WHEN bool_or(health='alive') THEN 'alive' \
                                WHEN bool_or(health='degraded') THEN 'degraded' \
                                ELSE 'dead' END AS health \
                    FROM node_status GROUP BY node_id) st ON st.node_id = n.id \
         LEFT JOIN node_payloads np ON np.node_id = n.id \
         WHERE n.membership='Linked'",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, protocol, server, port, score, region, health, nonce, ciphertext)| NodeRow {
                id,
                protocol,
                server,
                port,
                score,
                region: region.unwrap_or_default(),
                health: health.unwrap_or_else(|| "unknown".into()),
                nonce,
                ciphertext,
            },
        )
        .collect())
}

/// Decrypt one node payload. `None` = missing payload or unreadable (wrong key /
/// tampered bytes), which the caller counts instead of failing the whole request.
fn decrypt_payload(node: &NodeRow) -> Option<Value> {
    let nonce = node.nonce.as_ref()?;
    let ciphertext = node.ciphertext.as_ref()?;
    let plain = payload_crypto::global()
        .open(
            &payload_crypto::aad_for(&node.id.to_string()),
            nonce,
            ciphertext,
        )
        .ok()?;
    serde_json::from_slice::<Value>(&plain).ok()
}

fn pipeline_node(node: &NodeRow, payload: &Value) -> PipelineNode {
    let credential = payload
        .get("password")
        .or_else(|| payload.get("uuid"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    PipelineNode {
        id: node.id.to_string(),
        region: node.region.clone(),
        score: node.score,
        clash_proxy: payload.clone(),
        protocol: node.protocol.clone(),
        server: node.server.clone(),
        port: node.port.clamp(0, u16::MAX as i32) as u16,
        credential,
        tls_sni: payload
            .get("servername")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        ws_path: payload
            .get("ws-opts")
            .and_then(|v| v.get("path"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        // Streaming flags land with M7 (`node_streaming`); the filter AST treats an
        // empty map as "unknown" rather than "not unlocked".
        streaming: HashMap::new(),
    }
}

pub async fn sub(State(st): State<AppState>, Path(spec): Path<String>) -> Response {
    let Some((token, fmt)) = spec.rsplit_once('.') else {
        return err(StatusCode::NOT_FOUND, "expected /sub/{token}.{fmt}");
    };
    if !matches!(fmt, "yaml" | "json" | "base64") {
        return err(StatusCode::NOT_FOUND, "unknown format");
    }
    if fmt == "base64" {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "base64 requires real URI rendering (M1c); use .yaml or .json",
        );
    }

    let pepper = std::env::var("PHLOGO_TOKEN_PEPPER").unwrap_or_default();
    let hash = distribution::token::hash_token(token, &pepper);

    // Acceptance #3: unknown OR revoked tokens are rejected BEFORE any output.
    let Some(profile) = lookup_profile(st.pool(), &hash).await else {
        return err(StatusCode::UNAUTHORIZED, "unknown or revoked token");
    };

    let rows = match load_nodes(st.pool()).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")),
    };
    let mut nodes = Vec::with_capacity(rows.len());
    let mut unreadable = 0usize;
    for row in &rows {
        match decrypt_payload(row) {
            Some(payload) => nodes.push(pipeline_node(row, &payload)),
            None => unreadable += 1,
        }
    }
    if nodes.is_empty() {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("no usable node payloads ({unreadable} unreadable) — check PHLOGO_PAYLOAD_KEY"),
        );
    }

    let max_distribute: i64 =
        sqlx::query_scalar("SELECT value::bigint FROM configs WHERE key='max_distribute'")
            .fetch_optional(st.pool())
            .await
            .ok()
            .flatten()
            .unwrap_or(200);

    let thresholds = match crate::handlers::config::grade_thresholds(st.pool()).await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("config: {e}")),
    };

    // Sponsor text in ONE pass over the sponsors table (not one query per node).
    let health_of: HashMap<String, String> = rows
        .iter()
        .map(|r| (r.id.to_string(), r.health.clone()))
        .collect();
    let views: Vec<filter::NodeView> = nodes
        .iter()
        .map(|n| filter::NodeView {
            health: health_of
                .get(&n.id)
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
            region: Some(n.region.clone()),
            score: n.score,
            protocol: n.protocol.clone(),
            server: n.server.clone(),
            source_id: None,
            streaming: n.streaming.clone(),
        })
        .collect();
    let sponsor_texts = match registry::sponsor::match_texts(st.pool(), &views).await {
        Ok(v) => v,
        Err(_) => vec![String::new(); views.len()], // DB failure degrades to no sponsor
    };
    let index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();
    let sponsor_of = |n: &PipelineNode| -> String {
        index
            .get(&n.id)
            .and_then(|i| sponsor_texts.get(*i))
            .cloned()
            .unwrap_or_default()
    };

    let spec = ProfileSpec {
        filter_ast: profile.filter_ast,
        region: profile.region,
        health: profile.health,
        min_score: profile.min_score,
        limit_count: profile.limit_count.max(0) as usize,
        preset: profile.preset,
        naming_rule: profile.naming_rule,
    };

    let rendered = render_subscription_with_sponsor(
        &nodes,
        &spec,
        None,
        max_distribute.max(0) as usize,
        &thresholds,
        "",
        fmt,
        Some(&sponsor_of),
    );

    match rendered {
        Ok(RenderedSubscription::Mihomo(body)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
            body,
        )
            .into_response(),
        Ok(RenderedSubscription::Singbox(body)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            body,
        )
            .into_response(),
        Ok(RenderedSubscription::Base64(_)) => err(
            StatusCode::NOT_IMPLEMENTED,
            "base64 requires real URI rendering (M1c)",
        ),
        Err(msg) if msg.contains("no nodes matched") => err(StatusCode::NOT_FOUND, msg),
        Err(msg) => err(StatusCode::UNPROCESSABLE_ENTITY, msg),
    }
}
