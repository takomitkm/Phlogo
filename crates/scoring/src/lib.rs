//! Scoring engine (P4-03): faithful port of Phlogo `internal/scoring/scoring.go`
//! `computeScore` (EWMA baseline) + net-new version/components/insufficient_data.
//!
//! Decisions locked in P4 (documented in p4-03 page):
//! - speed branch follows the Go CODE baseline: `speed >= 0` is always true and the
//!   speed weight always mixes (untested speed = 0 kbps contributes scoreSpeed = 0);
//! - the authoritative jitter threshold is the config seed 200 ms (not the Go
//!   loadParams fallback of 70);
//! - recent-records query adds `, id DESC` tie-break for determinism.
pub mod engine;
pub mod params;
pub mod persist;

pub use engine::{
    compute_score, clamp01, DeepCheck, ScoreComponents, ScoreParams, ScoreResult, ScoreStatus,
};
pub use params::{load_params, min_samples};
pub use persist::{
    compute_and_update_tag_scores, recent_deep_checks, TagScoreInput, VERSION,
};

#[cfg(test)]
mod p4_tests;
