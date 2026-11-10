//! utoipa schema definitions. Paths live in `lib.rs` (utoipa's `#[path]` macro
//! resolves types in the enclosing scope, so both must see the same names).
use utoipa::ToSchema;

#[derive(ToSchema, serde::Deserialize)]
pub struct CreateSourceReq {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub info: String,
    pub content: String,
}

#[derive(ToSchema, serde::Deserialize)]
pub struct PatchSourceReq {
    pub status: Option<String>,
    pub identifier: Option<String>,
    pub info: Option<String>,
    pub content: Option<String>,
}

#[derive(ToSchema, serde::Serialize)]
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

#[derive(ToSchema, serde::Serialize)]
pub struct CreateSourceResp {
    pub id: Uuid,
    #[serde(rename = "alreadyExisted")]
    pub already_existed: bool,
    pub source: Option<SourceView>,
}

#[derive(ToSchema, serde::Serialize)]
pub struct NodeView {
    pub id: Uuid,
    pub identity_hash: String,
    pub fingerprint_version: i32,
    pub protocol: String,
    pub server: String,
    pub port: i32,
}

pub type Uuid = uuid::Uuid;

