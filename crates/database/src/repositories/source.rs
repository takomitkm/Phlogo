//! Source repository. Dedup by (type, content) is race-safe via the unique index
//! `uq_sources_type_content` + `INSERT ... ON CONFLICT DO NOTHING RETURNING`.
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::SourceRow;

const COLS: &str = r#"id, "type", identifier, info, content, status, ignore_dead, fetch_policy, created_at, updated_at"#;

#[derive(Debug, Clone)]
pub struct SourceRepository {
    pub pool: PgPool,
}

impl SourceRepository {
    /// Mirrors Phlogo `internal/source/service.go` Create: returns (id, already_existed).
    pub async fn create_dedup(
        &self,
        typ: &str,
        identifier: &str,
        info: &str,
        content: &str,
    ) -> anyhow::Result<(Uuid, bool)> {
        let new_id = Uuid::new_v4();
        let inserted: Option<Uuid> = sqlx::query_scalar(
            r#"INSERT INTO sources (id,"type",identifier,info,content,status,ignore_dead,fetch_policy,created_at,updated_at)
               VALUES ($1,$2,$3,$4,$5,'active',false,'replace',now(),now())
               ON CONFLICT ("type", content) DO NOTHING
               RETURNING id"#,
        )
        .bind(new_id)
        .bind(typ)
        .bind(identifier)
        .bind(info)
        .bind(content)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(id) = inserted {
            return Ok((id, false));
        }
        let existing: Uuid =
            sqlx::query_scalar(r#"SELECT id FROM sources WHERE "type"=$1 AND content=$2"#)
                .bind(typ)
                .bind(content)
                .fetch_one(&self.pool)
                .await?;
        Ok((existing, true))
    }

    pub async fn list(&self) -> anyhow::Result<Vec<SourceRow>> {
        let sql = format!(r#"SELECT {COLS} FROM sources ORDER BY created_at DESC"#);
        Ok(sqlx::query_as::<_, SourceRow>(&sql)
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<SourceRow>> {
        let sql = format!(r#"SELECT {COLS} FROM sources WHERE id=$1"#);
        Ok(sqlx::query_as::<_, SourceRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        let n = sqlx::query(r#"DELETE FROM sources WHERE id=$1"#)
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n > 0)
    }

    pub async fn set_ignore_dead(&self, id: Uuid, ignore: bool) -> anyhow::Result<bool> {
        let n = sqlx::query(r#"UPDATE sources SET ignore_dead=$2, updated_at=now() WHERE id=$1"#)
            .bind(id)
            .bind(ignore)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n > 0)
    }

    /// Partial update (PATCH): only provided fields change.
    pub async fn update(
        &self,
        id: Uuid,
        status: Option<&str>,
        identifier: Option<&str>,
        info: Option<&str>,
        content: Option<&str>,
    ) -> anyhow::Result<Option<SourceRow>> {
        let sql = format!(
            r#"UPDATE sources SET
                 status = COALESCE($2, status),
                 identifier = COALESCE($3, identifier),
                 info = COALESCE($4, info),
                 content = COALESCE($5, content),
                 updated_at = now()
               WHERE id=$1
               RETURNING {COLS}"#
        );
        Ok(sqlx::query_as::<_, SourceRow>(&sql)
            .bind(id)
            .bind(status)
            .bind(identifier)
            .bind(info)
            .bind(content)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Nodes linked to a source (via node_sources).
    pub async fn nodes_of_source(
        &self,
        source_id: Uuid,
    ) -> anyhow::Result<Vec<crate::models::NodeRow>> {
        Ok(sqlx::query_as::<_, crate::models::NodeRow>(
            r#"SELECT n.id,n.identity_hash,n.fingerprint_version,n.protocol,n.server,n.port,n.first_seen_at,n.last_seen_at
               FROM nodes n JOIN node_sources ns ON ns.node_id = n.id
               WHERE ns.source_id=$1 ORDER BY n.created_at DESC"#,
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?)
    }
}
