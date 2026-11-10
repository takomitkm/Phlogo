//! Source handlers (mirrors `internal/api/handler/source.go`). Dedup hit -> 200, new -> 201.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use database::SourceRow;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateSourceReq {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub info: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct PatchSourceReq {
    pub status: Option<String>,
    pub identifier: Option<String>,
    pub info: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SourceView {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub source_type: String,
    pub identifier: String,
    pub info: String,
    pub content: String,
    pub status: String,
    pub ignore_dead: bool,
    pub fetch_policy: String,
}

impl From<SourceRow> for SourceView {
    fn from(r: SourceRow) -> Self {
        Self {
            id: r.id,
            source_type: r.source_type,
            identifier: r.identifier,
            info: r.info,
            content: r.content,
            status: r.status,
            ignore_dead: r.ignore_dead,
            fetch_policy: r.fetch_policy,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateSourceResp {
    pub id: Uuid,
    #[serde(rename = "alreadyExisted")]
    pub already_existed: bool,
    pub source: Option<SourceView>,
}

fn err500(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response()
}

#[allow(clippy::result_large_err)]
fn parse_id(s: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(s).map_err(|_| (StatusCode::BAD_REQUEST, "invalid uuid").into_response())
}

pub async fn create(State(st): State<AppState>, Json(req): Json<CreateSourceReq>) -> Response {
    match st
        .sources
        .create(&req.source_type, &req.identifier, &req.info, &req.content)
        .await
    {
        Ok(c) => {
            let view = st
                .sources
                .get(c.id)
                .await
                .ok()
                .flatten()
                .map(SourceView::from);
            let code = if c.already_existed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (
                code,
                Json(CreateSourceResp {
                    id: c.id,
                    already_existed: c.already_existed,
                    source: view,
                }),
            )
                .into_response()
        }
        Err(e) => err500(e),
    }
}

pub async fn list(State(st): State<AppState>) -> Response {
    match st.sources.list().await {
        Ok(rows) => {
            let v: Vec<SourceView> = rows.into_iter().map(SourceView::from).collect();
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
    match st.sources.get(id).await {
        Ok(Some(row)) => Json(SourceView::from(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

pub async fn patch_source(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchSourceReq>,
) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match st
        .sources
        .update(
            id,
            req.status.as_deref(),
            req.identifier.as_deref(),
            req.info.as_deref(),
            req.content.as_deref(),
        )
        .await
    {
        Ok(Some(row)) => Json(SourceView::from(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match st.sources.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

pub async fn set_ignore_dead(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match st.sources.set_ignore_dead(id, true).await {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err500(e),
    }
}

pub async fn nodes(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match st.sources.nodes_of_source(id).await {
        Ok(rows) => {
            let v: Vec<super::node::NodeView> =
                rows.into_iter().map(super::node::NodeView::from).collect();
            Json(v).into_response()
        }
        Err(e) => err500(e),
    }
}
