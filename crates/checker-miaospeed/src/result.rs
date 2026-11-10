//! Result normalization + mapping (p3-03). Turns adapter-collected results into
//! `NormalizedCheckResult`s (stamping run/tag/time), dedups per node, and computes
//! unreported nodes (partial / timeout / disconnect handling).
use chrono::{DateTime, Utc};
use std::collections::{HashSet};
use uuid::Uuid;

use crate::types::{CheckResult, DeepResult, InitialResult};

/// Unit conventions (Phlogo / P0-02):
/// `latency_ms == -1` dead; `speed_kbps == 0` untested; ms/kbps everywhere.
#[derive(Debug, Clone)]
pub struct NormalizedCheckResult {
    pub node_id: Uuid,
    pub checker_tag: String,
    /// `check_runs.id`; idempotency/dedup key.
    pub run_id: Uuid,
    /// Late-result comparison key.
    pub checked_at: DateTime<Utc>,
    pub initial: InitialResult,
    pub deep: Option<DeepResult>,
    /// From `deep.region` (Phlogo `applyRegionUpdates` source); empty string -> None.
    pub region: Option<String>,
}

pub struct NormalizeContext {
    pub run_id: Uuid,
    pub checker_tag: String,
    /// Fallback timestamp when a result carries none.
    pub now: DateTime<Utc>,
}

/// Normalize collected results. The adapter already resolved `internal_node_id` from
/// `ProxyInfo.Name` (p3-02); results pointing at an unknown node cannot happen there,
/// so normalization only stamps context and applies unit defaults.
pub fn normalize(raw: &[CheckResult], ctx: &NormalizeContext) -> Vec<NormalizedCheckResult> {
    raw.iter()
        .map(|r| {
            let deep = r.deep.as_ref().map(|d| DeepResult {
                avg_latency_ms: d.avg_latency_ms,
                jitter_ms: d.jitter_ms,
                speed_kbps: d.speed_kbps,
                region: d.region.clone(),
                streaming: d.streaming.clone(),
                metadata: d.metadata.clone(),
            });
            NormalizedCheckResult {
                node_id: r.internal_node_id,
                checker_tag: ctx.checker_tag.clone(),
                run_id: ctx.run_id,
                checked_at: ctx.now,
                initial: InitialResult {
                    latency_ms: r.initial.latency_ms,
                },
                region: deep.as_ref().and_then(|d| d.region.clone()).filter(|s| !s.is_empty()),
                deep,
            }
        })
        .collect()
}

/// Per-run dedup by node_id (first wins) + partial detection.
/// Returns (deduped results, submitted-but-unreported node ids -> Unknown).
pub fn reconcile(
    submitted: &[Uuid],
    results: &[NormalizedCheckResult],
) -> (Vec<NormalizedCheckResult>, Vec<Uuid>) {
    let mut seen = HashSet::new();
    let dedup: Vec<NormalizedCheckResult> = results
        .iter()
        .filter(|r| seen.insert(r.node_id))
        .cloned()
        .collect();
    let reported: HashSet<Uuid> = dedup.iter().map(|r| r.node_id).collect();
    let unreported: Vec<Uuid> = submitted
        .iter()
        .filter(|n| !reported.contains(n))
        .copied()
        .collect();
    (dedup, unreported)
}
