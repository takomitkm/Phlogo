//! Lifecycle layer: state machines + atomic cleanup operations (P4).
pub mod ops;
pub mod state;
pub mod transitions;

pub use ops::{
    alive_window_minutes, check_history_days, cleanup_interval_minutes, cross_tag_alive_map,
    delete_retired_nodes, mark_dead_and_clean, prune_check_history, retire_expired_orphans,
    run_cleanup_cycle, CleanupReport,
};
pub use state::{FetchPolicy, NodeHealth, NodeMembership, SourceStatus};
pub use transitions::{can_transition_membership, can_transition_source, TransitionError};
