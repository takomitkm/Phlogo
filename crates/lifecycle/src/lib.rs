//! Lifecycle layer: state machines + atomic cleanup operations (P4).
pub mod ops;
pub mod state;
pub mod transitions;

pub use ops::{delete_retired_nodes, mark_dead_and_clean, retire_expired_orphans};
pub use state::{FetchPolicy, NodeHealth, NodeMembership, SourceStatus};
pub use transitions::{can_transition_membership, can_transition_source, TransitionError};
