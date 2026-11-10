//! Transition rule tables (plan §6.1/§6.3) + validation errors.
use crate::state::{NodeMembership, SourceStatus};

#[derive(Debug, thiserror::Error)]
pub enum TransitionError {
    #[error("invalid source transition: {0:?} -> {1:?}")]
    InvalidSourceTransition(SourceStatus, SourceStatus),
    #[error("invalid membership transition: {0:?} -> {1:?}")]
    InvalidMembershipTransition(NodeMembership, NodeMembership),
}

/// `Active <-> Suspended` reversible; both may become `Dead`; `Dead` is terminal
/// (idempotent on itself). Mirrors `MarkDeadAndClean`'s conditional-UPDATE guard.
pub fn can_transition_source(from: SourceStatus, to: SourceStatus) -> bool {
    use SourceStatus::*;
    matches!(
        (from, to),
        (Active, Suspended)
            | (Suspended, Active)
            | (Active, Dead)
            | (Suspended, Dead)
            | (Dead, Dead)
    )
}

/// `Linked -> Orphaned -> Retired`; re-reference revives (`Orphaned/Retired -> Linked`).
/// Physical deletion of `Retired` is a cleanup policy, not a state transition.
pub fn can_transition_membership(from: NodeMembership, to: NodeMembership) -> bool {
    use NodeMembership::*;
    matches!(
        (from, to),
        (Linked, Orphaned)
            | (Orphaned, Linked)
            | (Orphaned, Retired)
            | (Linked, Linked)
            | (Orphaned, Orphaned)
            | (Retired, Linked)
    )
}

pub fn transition_source(from: SourceStatus, to: SourceStatus) -> Result<(), TransitionError> {
    if can_transition_source(from, to) {
        Ok(())
    } else {
        Err(TransitionError::InvalidSourceTransition(from, to))
    }
}

pub fn transition_membership(
    from: NodeMembership,
    to: NodeMembership,
) -> Result<(), TransitionError> {
    if can_transition_membership(from, to) {
        Ok(())
    } else {
        Err(TransitionError::InvalidMembershipTransition(from, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_transitions() {
        use SourceStatus::*;
        assert!(can_transition_source(Active, Suspended));
        assert!(can_transition_source(Suspended, Active));
        assert!(can_transition_source(Active, Dead));
        assert!(can_transition_source(Suspended, Dead));
        assert!(can_transition_source(Dead, Dead), "idempotent");
        assert!(!can_transition_source(Dead, Active), "dead is terminal");
        assert!(!can_transition_source(Dead, Suspended));
        assert!(!can_transition_source(Active, Active), "not in rule table");
    }

    #[test]
    fn membership_transitions() {
        use NodeMembership::*;
        assert!(can_transition_membership(Linked, Orphaned));
        assert!(can_transition_membership(Orphaned, Retired));
        assert!(can_transition_membership(Orphaned, Linked), "revival");
        assert!(can_transition_membership(Retired, Linked), "pre-cleanup revival");
        assert!(can_transition_membership(Linked, Linked), "idempotent");
        assert!(can_transition_membership(Orphaned, Orphaned));
        assert!(!can_transition_membership(Linked, Retired));
        assert!(!can_transition_membership(Retired, Orphaned));
        assert!(!can_transition_membership(Retired, Retired));
    }
}
