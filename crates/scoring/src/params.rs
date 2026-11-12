//! Parameter loading from `configs` (scoring.go `loadParams` equivalent; per-call read
//! replaces Go's 1-min TTL cache — parameter changes apply at the next recompute).
//!
//! **Unit contract (fixed 2026-11-13)**: ratio keys are stored as **decimals** (`0.7`),
//! which is what `migrations` seeds and what `docs/configuration-and-api.md` documents.
//! The previous reader parsed them as percent integers (`70`) and silently fell back to
//! the in-code default, so edits never took effect. Percent integers are still accepted
//! for compatibility (`70` → `0.7`); anything ≥ 2 is read as a percentage.
use sqlx::PgPool;

use crate::engine::ScoreParams;

async fn cfg_raw(pool: &PgPool, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM configs WHERE key=$1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Read a ratio: `0.7` → 0.7, `70` → 0.7 (percent compatibility). Unparsable → fallback.
async fn cfg_ratio(pool: &PgPool, key: &str, fallback: f64) -> f64 {
    match cfg_raw(pool, key).await.and_then(|s| s.trim().parse::<f64>().ok()) {
        Some(v) if v.is_finite() && v >= 0.0 => {
            if v >= 2.0 {
                v / 100.0
            } else {
                v
            }
        }
        _ => fallback,
    }
}

async fn cfg_i64(pool: &PgPool, key: &str, fallback: i64) -> i64 {
    cfg_raw(pool, key)
        .await
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(fallback)
}

pub async fn load_params(pool: &PgPool) -> anyhow::Result<ScoreParams> {
    let alpha = cfg_ratio(pool, "score_decay_alpha", 0.7).await;
    let w_latency = cfg_ratio(pool, "score_weight_latency", 0.7).await;
    let w_stability = cfg_ratio(pool, "score_weight_stability", 0.3).await;
    let w_speed = cfg_ratio(pool, "score_weight_speed", 0.55).await;
    let latency_curve_k = cfg_ratio(pool, "score_latency_curve_k", 0.3).await;
    let jitter = cfg_i64(pool, "score_jitter_linear_threshold_ms", 200).await;
    Ok(ScoreParams {
        alpha,
        w_latency,
        w_stability,
        w_speed,
        latency_curve_k,
        jitter_linear_threshold_ms: jitter as f64,
    })
}

pub async fn min_samples(pool: &PgPool) -> anyhow::Result<usize> {
    Ok(cfg_i64(pool, "score_min_samples", 1).await.max(1) as usize)
}

#[cfg(test)]
mod tests {
    /// The unit rule itself, without a database: seeded decimals and legacy
    /// percent integers must both resolve to the same ratio.
    fn ratio(v: f64) -> f64 {
        if v >= 2.0 {
            v / 100.0
        } else {
            v
        }
    }

    #[test]
    fn decimals_and_percents_agree() {
        assert!((ratio(0.7) - 0.7).abs() < 1e-12);
        assert!((ratio(70.0) - 0.7).abs() < 1e-12);
        assert!((ratio(0.3) - 0.3).abs() < 1e-12);
        assert!((ratio(30.0) - 0.3).abs() < 1e-12);
        assert!((ratio(0.55) - 0.55).abs() < 1e-12);
        assert!((ratio(55.0) - 0.55).abs() < 1e-12);
    }

    #[test]
    fn curve_k_keeps_small_values() {
        // k must stay below 2 (above that the value is read as a percentage).
        assert!((ratio(1.5) - 1.5).abs() < 1e-12);
    }
}
