//! Subscription HTTP endpoint (P7-03): `GET /sub/{token}.{fmt}` with hash-verified,
//! revocable tokens (acceptance #3). Node selection reuses the p5-02 selector over
//! alive candidates; full profile/filter wiring lands with the candidates endpoint.
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use sqlx::PgPool;

use crate::state::AppState;

fn err(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

/// token hash -> profile version, only for active tokens on enabled profiles.
async fn lookup_profile(
    pool: &PgPool,
    token_hash: &str,
) -> Option<(Option<serde_json::Value>, String, i64)> {
    sqlx::query_as(
        "SELECT p.filter_ast, p.preset, p.version FROM subscription_tokens t \
         JOIN subscription_profiles p ON p.id = t.profile_id \
         WHERE t.token_hash=$1 AND t.status='active' AND p.enabled",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn sub(State(st): State<AppState>, Path(spec): Path<String>) -> Response {
    let Some((token, fmt)) = spec.rsplit_once('.') else {
        return err(StatusCode::NOT_FOUND, "expected /sub/{token}.{fmt}");
    };
    if !matches!(fmt, "yaml" | "json" | "base64") {
        return err(StatusCode::NOT_FOUND, "unknown format");
    }

    let pepper = std::env::var("PAILY_TOKEN_PEPPER").unwrap_or_default();
    let hash = distribution::token::hash_token(token, &pepper);

    // Acceptance #3: unknown OR revoked tokens are rejected BEFORE any output.
    let Some((_filter_ast, preset, _version)) = lookup_profile(st.pool(), &hash).await else {
        return err(StatusCode::UNAUTHORIZED, "unknown or revoked token");
    };
    let _ = preset;

    // Candidate set: alive linked nodes with their score (selector input).
    let rows: Result<Vec<(uuid::Uuid, String, f64)>, _> = sqlx::query_as(
        "SELECT n.id, COALESCE(NULLIF(n.membership,''),'Linked'), COALESCE(s.score, 0) \
         FROM nodes n LEFT JOIN node_scores s ON s.node_id=n.id \
         WHERE n.membership='Linked'",
    )
    .fetch_all(st.pool())
    .await;
    let Ok(rows) = rows else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "db error");
    };
    let candidates: Vec<distribution::Candidate> = rows
        .into_iter()
        .map(|(id, region, score)| distribution::Candidate {
            id: id.to_string(),
            region,
            score,
        })
        .collect();

    let max_distribute: i64 = sqlx::query_scalar(
        "SELECT value::bigint FROM configs WHERE key='max_distribute'",
    )
    .fetch_optional(st.pool())
    .await
    .ok()
    .flatten()
    .unwrap_or(200);

    let mut rng = {
        use rand::SeedableRng;
        rand::rngs::StdRng::from_entropy()
    };
    let selected = distribution::select_with_preset(&candidates, max_distribute.max(0) as usize, "common", &mut rng);
    if selected.is_empty() {
        return err(StatusCode::NOT_FOUND, "no nodes matched");
    }

    // Naming (default template; full payload conversion lands with node_payloads).
    // Grade thresholds come from `configs` (B3 gap: `grade_*_min` had no consumer).
    let thresholds = match crate::handlers::config::grade_thresholds(st.pool()).await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
    };
    // Sponsor text per node (B3 gap #2): priority-ascending match over the sponsors
    // table, `default_sponsor_text` fallback. The view mirrors what the sponsor
    // filter can reference (health unknown until checks run; no streaming yet).
    let mut sponsor_of: std::collections::HashMap<String, String> = Default::default();
    for s in &selected {
        let view = filter::NodeView {
            health: "unknown".into(),
            region: Some(s.candidate.region.clone()),
            score: s.candidate.score,
            protocol: String::new(),
            server: String::new(),
            source_id: None,
            streaming: Default::default(),
        };
        // DB error degrades to the default text, never fails the subscription.
        let text = registry::sponsor::match_text(st.pool(), &view)
            .await
            .unwrap_or_default();
        sponsor_of.insert(s.candidate.id.clone(), text);
    }
    let lines: Vec<String> = selected
        .iter()
        .map(|s| {
            let mut f = distribution::naming::fields_for(s, &thresholds, "");
            f.sponsor = sponsor_of.get(&s.candidate.id).cloned().unwrap_or_default();
            format!("{}://node/{}", s.candidate.id, distribution::naming::render(distribution::naming::DEFAULT_TEMPLATE, &f))
        })
        .collect();

    match fmt {
        "yaml" => {
            let proxies: Vec<serde_json::Value> = selected
                .iter()
                .map(|s| {
                    let mut f = distribution::naming::fields_for(s, &thresholds, "");
                    f.sponsor = sponsor_of.get(&s.candidate.id).cloned().unwrap_or_default();
                    let name = distribution::naming::render(distribution::naming::DEFAULT_TEMPLATE, &f);
                    serde_json::json!({
                        "name": name,
                        "type": "trojan", "server": "placeholder.invalid", "port": 443, "password": ""
                    })
                })
                .collect();
            let body = format!(
                "mixed-port: 7890\nmode: rule\nproxies:\n{}",
                serde_yaml::to_string(&proxies).unwrap_or_default()
            );
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
                body,
            )
                .into_response()
        }
        "json" => {
            let body = serde_json::json!({
                "outbounds": lines.iter().map(|l| serde_json::json!({"type": "direct", "tag": l})).collect::<Vec<_>>(),
            });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                body.to_string(),
            )
                .into_response()
        }
        _ => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(lines.join("\n"));
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                b64,
            )
                .into_response()
        }
    }
}
