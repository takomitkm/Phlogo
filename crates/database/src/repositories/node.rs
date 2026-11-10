//! Node repository. `upsert_by_hash` mirrors Phlogo `internal/node/upsert.go`
//! (identity_hash hit reuses the node; `link` is idempotent).
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::NodeRow;

const COLS: &str =
    "id, identity_hash, fingerprint_version, protocol, server, port, first_seen_at, last_seen_at";

#[derive(Debug, Clone)]
pub struct NodeRepository {
    pub pool: PgPool,
}

impl NodeRepository {
    /// Returns (node_id, already_existed) by identity_hash.
    pub async fn upsert_by_hash(
        &self,
        identity_hash: &str,
        fingerprint_version: i32,
        protocol: &str,
        server: &str,
        port: i32,
    ) -> anyhow::Result<(Uuid, bool)> {
        let new_id = Uuid::new_v4();
        let inserted: Option<Uuid> = sqlx::query_scalar(
            r#"INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,first_seen_at,last_seen_at,created_at,updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,now(),now(),now(),now())
               ON CONFLICT (identity_hash) DO NOTHING
               RETURNING id"#,
        )
        .bind(new_id)
        .bind(identity_hash)
        .bind(fingerprint_version)
        .bind(protocol)
        .bind(server)
        .bind(port)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(id) = inserted {
            return Ok((id, false));
        }
        let existing: Uuid = sqlx::query_scalar("SELECT id FROM nodes WHERE identity_hash=$1")
            .bind(identity_hash)
            .fetch_one(&self.pool)
            .await?;
        Ok((existing, true))
    }

    /// Idempotent node<->source link; returns rows affected (0 = already linked).
    pub async fn link(&self, node_id: Uuid, source_id: Uuid) -> anyhow::Result<u64> {
        let n = sqlx::query(
            "INSERT INTO node_sources (node_id,source_id,first_seen_at,last_seen_at)
             VALUES ($1,$2,now(),now()) ON CONFLICT (node_id,source_id) DO NOTHING",
        )
        .bind(node_id)
        .bind(source_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<NodeRow>> {
        let sql = format!("SELECT {COLS} FROM nodes ORDER BY created_at DESC");
        Ok(sqlx::query_as::<_, NodeRow>(&sql)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<NodeRow>> {
        let sql = format!("SELECT {COLS} FROM nodes WHERE id=$1");
        Ok(sqlx::query_as::<_, NodeRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        let n = sqlx::query("DELETE FROM nodes WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n > 0)
    }

    pub async fn count_node_sources(&self, node_id: Uuid) -> anyhow::Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM node_sources WHERE node_id=$1")
                .bind(node_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}
