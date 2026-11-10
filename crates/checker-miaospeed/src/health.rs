//! Per-tag health snapshots + cross-tag alive merge (p3-03 Steps 4-5).
//! Pure functions mirroring Phlogo `batchAliveDecision`; persistence lands in p3-04.
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::result::NormalizedCheckResult;

/// Plan §6.2 health states. `Degraded` is a P4 (scoring) concern; not derived here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHealth {
    Unknown,
    Alive,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerTagHealth {
    pub node_id: Uuid,
    pub checker_tag: String,
    pub health: NodeHealth,
    pub last_checked_at: DateTime<Utc>,
}

/// Single result -> per-tag health (`latency_ms != -1` == alive, Phlogo rule).
pub fn health_from_result(r: &NormalizedCheckResult) -> NodeHealth {
    if r.initial.latency_ms == -1 {
        NodeHealth::Dead
    } else {
        NodeHealth::Alive
    }
}

/// Build a per-tag snapshot from one result.
pub fn snapshot_from_result(r: &NormalizedCheckResult) -> PerTagHealth {
    PerTagHealth {
        node_id: r.node_id,
        checker_tag: r.checker_tag.clone(),
        health: health_from_result(r),
        last_checked_at: r.checked_at,
    }
}

/// Late-result rule: overwrite only when strictly newer; `false` = late, drop it.
pub fn should_overwrite(existing: Option<&PerTagHealth>, incoming: &NormalizedCheckResult) -> bool {
    match existing {
        None => true,
        Some(e) => incoming.checked_at > e.last_checked_at,
    }
}

/// Cross-tag merge: alive iff ANY tag reported alive within the window
/// (`node_alive_check_window_minutes`, default 240).
pub fn cross_tag_alive(snapshots: &[PerTagHealth], window_minutes: i64, now: DateTime<Utc>) -> bool {
    let cutoff = now - chrono::Duration::minutes(window_minutes);
    snapshots
        .iter()
        .any(|s| s.last_checked_at >= cutoff && s.health == NodeHealth::Alive)
}
