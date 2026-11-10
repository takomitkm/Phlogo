//! P3-01 integration tests (require DATABASE_URL; skipped otherwise).
use sqlx::PgPool;
use uuid::Uuid;
use worker::{finish_check_run, record_check_run, CheckJob};

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn job() -> CheckJob {
    CheckJob {
        run_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        node_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        presets: vec!["idle".into()],
    }
}

/// Acceptance #2/#3: duplicate delivery must not duplicate business side effects.
#[tokio::test]
async fn no_duplicate_work() {
    let Some(pool) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let j = job();
    assert!(record_check_run(&pool, &j).await.unwrap(), "first insert wins");
    for _ in 0..3 {
        assert!(
            !record_check_run(&pool, &j).await.unwrap(),
            "redelivery must not create a second row"
        );
    }
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM check_runs WHERE id=$1")
        .bind(j.run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1, "exactly one check_runs row per run_id");

    finish_check_run(&pool, j.run_id).await.unwrap();
    finish_check_run(&pool, j.run_id).await.unwrap();
    let (status, finishes): (String, i64) = sqlx::query_as(
        "SELECT status, count(*) FILTER (WHERE finished_at IS NOT NULL) FROM check_runs WHERE id=$1 GROUP BY status",
    )
    .bind(j.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "finished");
    assert_eq!(finishes, 1, "finish transition must be one-shot");
}
