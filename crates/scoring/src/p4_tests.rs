//! P4-03 acceptance tests. A/B/C are pure; D/E need DATABASE_URL (skipped otherwise).
use crate::{clamp01, compute_score, DeepCheck, ScoreParams, ScoreStatus};

fn params() -> ScoreParams {
    ScoreParams {
        alpha: 0.7,
        w_latency: 0.7,
        w_stability: 0.3,
        w_speed: 0.55,
        latency_curve_k: 0.3,
        jitter_linear_threshold_ms: 200.0,
    }
}

/// Acceptance A: EWMA golden fixture (hand-computed in p4-03 page).
#[test]
fn ewma_baseline_fixture() {
    let records = vec![
        DeepCheck { avg_latency_ms: 100, jitter_ms: 20, speed_kbps: 163840 },
        DeepCheck { avg_latency_ms: 200, jitter_ms: 40, speed_kbps: 0 },
        DeepCheck { avg_latency_ms: 500, jitter_ms: 100, speed_kbps: 0 },
    ];
    let out = compute_score(&records, &params());
    assert!(
        (out.score - 0.967722).abs() < 1e-6,
        "golden fixture mismatch: {}",
        out.score
    );
    assert_eq!(out.status, ScoreStatus::Ok);
    assert_eq!(out.version, "ewma-v1");
    let c = &out.components;
    assert!((c.weighted_latency_ms - 485.0 / 2.19).abs() < 1e-9);
    assert!((c.weighted_jitter_ms - 97.0 / 2.19).abs() < 1e-9);
    assert!((c.quality_score - 0.928270).abs() < 1e-5);
    assert_eq!(c.speed_score, Some(1.0));
    assert_eq!(c.sample_count, 3);
}

/// Acceptance B: unreachable record maps to worst case; no panic/division by zero.
#[test]
fn unreachable_record_worst_case() {
    let records = vec![DeepCheck { avg_latency_ms: -1, jitter_ms: -1, speed_kbps: 0 }];
    let out = compute_score(&records, &params());
    // latencyX=1 -> latencyScore=0; jitterX=1 -> stability=(1-t)*sqrt(0)=0; quality=0;
    // speedX=0 -> speedScore=0 => score = 0.
    assert!(out.score.abs() < 1e-9, "worst case must score ~0, got {}", out.score);
}

/// Acceptance C: result rounds to 6 decimals.
#[test]
fn rounds_to_6_decimals() {
    let records: Vec<DeepCheck> = (0..7)
        .map(|i| DeepCheck { avg_latency_ms: 37 + i, jitter_ms: 11 + i, speed_kbps: 1000 + i })
        .collect();
    let out = compute_score(&records, &params());
    let scaled = out.score * 1e6;
    assert!((scaled - scaled.round()).abs() < 1e-6, "must be 6-decimal rounded");
    assert!(clamp01(1.5) == 1.0 && clamp01(-0.1) == 0.0);
}

/// Acceptance E: version + insufficient data.
#[test]
fn returns_version_and_insufficient_data() {
    let out = compute_score(&[], &params());
    assert_eq!(out.status, ScoreStatus::InsufficientData);
    assert_eq!(out.score, 0.0);
    assert_eq!(out.version, "ewma-v1");
}

/// Acceptance D: nodes.score AVG ignores zero scores.
#[tokio::test]
async fn node_avg_ignores_zero_scores() {
    let dsn = match std::env::var("DATABASE_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("skip: DATABASE_URL not set");
            return;
        }
    };
    let pool = database::connect(&dsn).await.unwrap();
    database::migrate(&pool).await.unwrap();
    let node = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO nodes (id,identity_hash,fingerprint_version,protocol,server,port,first_seen_at,last_seen_at,created_at,updated_at,score) \
         VALUES ($1,$2,1,'trojan','t.example.com',443,now(),now(),now(),now(),0)",
    )
    .bind(node)
    .bind(Uuid::new_v4().simple().to_string())
    .execute(&pool)
    .await
    .unwrap();

    let inputs = vec![
        crate::TagScoreInput {
            node_id: node,
            checker_tag: "tagA".into(),
            records: vec![DeepCheck { avg_latency_ms: 100, jitter_ms: 20, speed_kbps: 163840 }],
        },
        crate::TagScoreInput {
            node_id: node,
            checker_tag: "tagB".into(),
            records: vec![DeepCheck { avg_latency_ms: -1, jitter_ms: -1, speed_kbps: 0 }],
        },
    ];
    crate::compute_and_update_tag_scores(&pool, &inputs, &params()).await.unwrap();

    let score: f64 = sqlx::query_scalar("SELECT score FROM nodes WHERE id=$1")
        .bind(node)
        .fetch_one(&pool)
        .await
        .unwrap();
    let tag_a: f64 = sqlx::query_scalar(
        "SELECT score FROM node_scores WHERE node_id=$1 AND checker_tag='tagA'",
    )
    .bind(node)
    .fetch_one(&pool)
    .await
    .unwrap();
    let tag_b: f64 = sqlx::query_scalar(
        "SELECT score FROM node_scores WHERE node_id=$1 AND checker_tag='tagB'",
    )
    .bind(node)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(tag_a > 0.9, "good tag must score high, got {tag_a}");
    assert!(tag_b.abs() < 1e-9, "dead tag must score 0, got {tag_b}");
    assert!(
        (score - tag_a).abs() < 1e-9,
        "AVG must ignore zero scores: nodes.score={score}, tagA={tag_a}"
    );
}

use uuid::Uuid;
