//! Node handlers (P1: list/get/delete; nodes are otherwise created by fetch in P2).
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use database::NodeRow;
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct NodeView {
    pub id: Uuid,
    pub identity_hash: String,
    pub fingerprint_version: i32,
    pub protocol: String,
    pub server: String,
    pub port: i32,
}

impl From<NodeRow> for NodeView {
    fn from(r: NodeRow) -> Self {
        Self {
            id: r.id,
            identity_hash: r.identity_hash,
            fingerprint_version: r.fingerprint_version,
            protocol: r.protocol,
            server: r.server,
            port: r.port,
        }
    }
}

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

#[allow(clippy::result_large_err)]
fn parse_id(s: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(s).map_err(|_| (StatusCode::BAD_REQUEST, "invalid uuid").into_response())
}

pub async fn list(State(st): State<AppState>) -> Response {
    match st.nodes.list().await {
        Ok(rows) => {
            let v: Vec<NodeView> = rows.into_iter().map(NodeView::from).collect();
            Json(v).into_response()
        }
        Err(e) => err500(e),
    }
}

pub async fn get(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match st.nodes.get(id).await {
        Ok(Some(row)) => Json(NodeView::from(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match st.nodes.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}
