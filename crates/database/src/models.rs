//! Row models for P1 tables (sources/nodes).
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct SourceRow {
    pub id: Uuid,
    #[sqlx(rename = "type")]
    pub source_type: String,
    pub identifier: String,
    pub info: String,
    pub content: String,
    pub status: String,
    pub ignore_dead: bool,
    pub fetch_policy: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct NodeRow {
    pub id: Uuid,
    pub identity_hash: String,
    pub fingerprint_version: i32,
    pub protocol: String,
    pub server: String,
    pub port: i32,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub membership: String,
    pub orphaned_at: Option<DateTime<Utc>>,
}
