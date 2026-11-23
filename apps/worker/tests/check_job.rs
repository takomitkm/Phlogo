//! P3-01 integration tests (require DATABASE_URL; skipped otherwise).
use sqlx::PgPool;
use uuid::Uuid;
use worker::{claim_for_exec, record_check_run, CheckJob};

async fn setup() -> Option<PgPool> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = database::connect(&dsn).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn job() -> CheckJob {
    CheckJob {
        run_id: Uuid::new_v4(),
        schedule_id: Uuid::new_v4(),
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
    assert!(
        record_check_run(&pool, &j).await.unwrap(),
        "first insert wins"
    );
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

    // The terminal transition is expressed through the EXECUTION LEASE (M3.1-3):
    // the worker claims the run, then finishes it under that owner.
    //
    // (This used to call the unguarded `finish_check_run`, which was exported from the
    // worker crate — a foot-gun: anyone could mark a leased run `success` and leave a
    // stale `exec_owner`. That export is gone; the guarded form is the only way.)
    let owner = claim_for_exec(&pool, j.run_id)
        .await
        .expect("claim")
        .expect("claimable");
    let mut tx = pool.begin().await.expect("tx");
    database::check::finish_check_run_owned_in(&mut tx, j.run_id, owner)
        .await
        .expect("finish");
    tx.commit().await.expect("commit");

    // A SECOND terminal write with the same owner must not apply: the first one cleared
    // `exec_owner`, so the owner guard no longer matches. Note the semantics: it is NOT
    // a silent no-op — the guarded form reports it as an error (`ensure!(affected==1)`),
    // because "the terminal state was already written" is a caller bug, not a normal
    // outcome.
    let mut tx2 = pool.begin().await.expect("tx2");
    let second = database::check::finish_check_run_owned_in(&mut tx2, j.run_id, owner).await;
    assert!(
        second.is_err(),
        "a second terminal write under the same owner must be refused, not applied \
         again (got {second:?})"
    );
    tx2.rollback().await.ok();

    let (status, finishes): (String, i64) = sqlx::query_as(
        "SELECT status, count(*) FILTER (WHERE finished_at IS NOT NULL) FROM check_runs WHERE id=$1 GROUP BY status",
    )
    .bind(j.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "success");
    assert_eq!(finishes, 1, "finish transition must be one-shot");

    // S81: remove the row this test created — it used to be left behind in the shared
    // dev database on every run.
    sqlx::query("DELETE FROM check_runs WHERE id=$1")
        .bind(j.run_id)
        .execute(&pool)
        .await
        .expect("delete run row");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM check_runs WHERE id=$1")
        .bind(j.run_id)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    assert_eq!(left, 0, "the test left its run row behind");
}
