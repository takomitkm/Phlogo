//! Persistence: `ComputeAndUpdateTagScore` port (scoring.go:59-96).
//! Writes `node_scores` per (node, tag), then `nodes.score = AVG(score>0)`.
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::{compute_score, DeepCheck, ScoreParams, SCORE_VERSION};

/// One tag-scoped scoring input row (newest-first per (node, tag)).
pub struct TagScoreInput {
    pub node_id: Uuid,
    pub checker_tag: String,
    /// Newest-first deep-check records (max 20).
    pub records: Vec<DeepCheck>,
}

/// Recompute + persist scores for the given (node, tag) pairs, then refresh nodes.score.
pub async fn compute_and_update_tag_scores(
    pool: &PgPool,
    inputs: &[TagScoreInput],
    params: &ScoreParams,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for input in inputs {
        let result = compute_score(&input.records, params);
        sqlx::query(
            "INSERT INTO node_scores (node_id,checker_tag,score,components,version,calculated_at) \
             VALUES ($1,$2,$3,$4,$5,now()) \
             ON CONFLICT (node_id,checker_tag) DO UPDATE \
               SET score=EXCLUDED.score, components=EXCLUDED.components, \
                   version=EXCLUDED.version, calculated_at=now()",
        )
        .bind(input.node_id)
        .bind(&input.checker_tag)
        .bind(result.score)
        .bind(serde_json::json!({
            "weighted_latency_ms": result.components.weighted_latency_ms,
            "weighted_jitter_ms": result.components.weighted_jitter_ms,
            "latency_score": result.components.latency_score,
            "stability_score": result.components.stability_score,
            "quality_score": result.components.quality_score,
            "speed_score": result.components.speed_score,
            "sample_count": result.components.sample_count,
        }))
        .bind(&result.version)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // nodes.score = AVG(score>0) per node (only positive tag scores participate).
    sqlx::query(
        "UPDATE nodes SET score = COALESCE(agg.avg_score, 0), updated_at=now() \
         FROM (SELECT node_id, AVG(score) AS avg_score FROM node_scores WHERE score > 0 GROUP BY node_id) agg \
         WHERE nodes.id = agg.node_id",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Load the newest 20 deep-check records for a (node, tag) pair (newest-first,
/// `id DESC` tie-break for determinism — plan §15 risk fix).
pub async fn recent_deep_checks(
    pool: &PgPool,
    node_id: Uuid,
    tag: &str,
) -> anyhow::Result<Vec<DeepCheck>> {
    let rows: Vec<(f64, f64, f64)> = sqlx::query_as(
        "SELECT (metrics->>'avg_latency_ms')::float8, \
                COALESCE(metrics->>'jitter_ms','0')::float8, \
                COALESCE(metrics->>'speed_kbps','0')::float8 \
         FROM node_check_results \
         WHERE node_id=$1 AND checker_tag=$2 AND result_status='alive' \
         ORDER BY checked_at DESC, id DESC LIMIT 20",
    )
    .bind(node_id)
    .bind(tag)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(lat, jit, speed)| DeepCheck {
            avg_latency_ms: lat as i32,
            jitter_ms: jit as i32,
            speed_kbps: speed as i32,
        })
        .collect())
}

/// Version string stamped by this build.
pub const VERSION: &str = SCORE_VERSION;
