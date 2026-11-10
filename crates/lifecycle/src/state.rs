//! The four lifecycle enums (plan §6.1-§6.3). `Suspended`, `FetchPolicy`, and
//! `Orphaned`/`Retired` are net-new relative to Phlogo (which only has active/dead
//! and physically deletes orphans immediately).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceStatus {
    Active,
    Suspended,
    Dead,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceStatus::Active => "active",
            SourceStatus::Suspended => "suspended",
            SourceStatus::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FetchPolicy {
    Replace,
    Accumulate,
}

impl FetchPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            FetchPolicy::Replace => "replace",
            FetchPolicy::Accumulate => "accumulate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeHealth {
    Unknown,
    Alive,
    /// net-new; derived by the scoring layer (p4-03).
    Degraded,
    Dead,
}

impl NodeHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeHealth::Unknown => "unknown",
            NodeHealth::Alive => "alive",
            NodeHealth::Degraded => "degraded",
            NodeHealth::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeMembership {
    Linked,
    Orphaned,
    Retired,
}

impl NodeMembership {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeMembership::Linked => "Linked",
            NodeMembership::Orphaned => "Orphaned",
            NodeMembership::Retired => "Retired",
        }
    }
}
