//! P3-03 acceptance tests (pure functions; no DB / network).
use crate::types::{CheckResult, InitialResult};
use crate::{
    cross_tag_alive, health_from_result, normalize, reconcile, should_overwrite,
    snapshot_from_result, NormalizeContext, NodeHealth,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use uuid::Uuid;

fn t(offset_min: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 0, 0, 0).unwrap() + Duration::minutes(offset_min)
}

fn ctx(tag: &str, at: DateTime<Utc>) -> NormalizeContext {
    NormalizeContext {
        run_id: Uuid::new_v4(),
        checker_tag: tag.into(),
        now: at,
    }
}

fn result(node: Uuid, latency: i32) -> CheckResult {
    CheckResult {
        internal_node_id: node,
        initial: InitialResult { latency_ms: latency },
        deep: None,
    }
}

/// Acceptance #1: every normalized result maps to the submitted node id.
#[test]
fn maps_each_result_to_correct_node_id() {
    let n1 = Uuid::new_v4();
    let n2 = Uuid::new_v4();
    let raw = vec![result(n1, 42), result(n2, 100)];
    let out = normalize(&raw, &ctx("default", t(0)));
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].node_id, n1);
    assert_eq!(out[1].node_id, n2);
    assert_eq!(out[0].initial.latency_ms, 42);
    assert_eq!(out[0].checker_tag, "default");
    assert!(out[0].region.is_none());
}

/// Acceptance #2: late results never overwrite a newer snapshot.
#[test]
fn late_result_does_not_overwrite_newer_snapshot() {
    let n = Uuid::new_v4();
    let newer = snapshot_from_result(&normalize(&[result(n, 50)], &ctx("a", t(10)))[0]);
    let late = normalize(&[result(n, 70)], &ctx("a", t(0)))[0].clone();
    assert!(!should_overwrite(Some(&newer), &late), "late result must be dropped");
    assert_eq!(newer.health, NodeHealth::Alive);

    let older = snapshot_from_result(&normalize(&[result(n, 50)], &ctx("a", t(0)))[0]);
    let fresh = normalize(&[result(n, 70)], &ctx("a", t(10)))[0].clone();
    assert!(should_overwrite(Some(&older), &fresh), "newer result must overwrite");
    // First-ever write always wins.
    assert!(should_overwrite(None, &fresh));
}

/// Acceptance #3: partial (unreported->Unknown set), duplicates collapse, reconnect identical.
#[test]
fn partial_and_duplicate_and_reconnect() {
    let submitted: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
    // Only 3 of 5 reported; node[1] reported twice (duplicate after reconnect).
    let mut raw = vec![
        result(submitted[0], 10),
        result(submitted[1], 20),
        result(submitted[2], -1),
    ];
    raw.push(result(submitted[1], 20));
    let out = normalize(&raw, &ctx("default", t(0)));
    let (dedup, unreported) = reconcile(&submitted, &out);
    assert_eq!(dedup.len(), 3, "duplicates collapse to one per node");
    assert_eq!(unreported, vec![submitted[3], submitted[4]], "unreported nodes surfaced");
    let dead = dedup.iter().find(|r| r.node_id == submitted[2]).unwrap();
    assert_eq!(health_from_result(dead), NodeHealth::Dead);
}

/// Acceptance #4: cross-tag alive merge within the window.
#[test]
fn cross_tag_alive_merge() {
    let n = Uuid::new_v4();
    let mk = |tag: &str, latency: i32, at: DateTime<Utc>| {
        snapshot_from_result(&normalize(&[result(n, latency)], &ctx(tag, at))[0])
    };
    let now = t(1000);
    let window = 240;

    // tag A dead, tag B alive in-window => alive.
    let snaps = vec![mk("a", -1, t(990)), mk("b", 30, t(995))];
    assert!(cross_tag_alive(&snaps, window, now));

    // both dead in-window => dead.
    let snaps = vec![mk("a", -1, t(990)), mk("b", -1, t(995))];
    assert!(!cross_tag_alive(&snaps, window, now));

    // alive but outside the window => dead.
    let snaps = vec![mk("b", 30, t(700))];
    assert!(!cross_tag_alive(&snaps, window, now));

    // alive at the exact cutoff boundary => alive.
    let snaps = vec![mk("b", 30, t(760))];
    assert!(cross_tag_alive(&snaps, window, now));
}
