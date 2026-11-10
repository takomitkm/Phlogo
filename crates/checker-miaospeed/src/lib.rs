//! MiaoSpeed WebSocket adapter (P3-02). Implements the plan §8 `Checker` trait over the
//! protocol frozen by P0-02 (tag 4.7.9): inline per-request auth via `hashMiaoSpeed`,
//! the `Clone()` Vendor-drop quirk, byte-exact jsoniter-shaped serialization, and
//! `ProxyInfo.Name`-based result mapping.
pub mod adapter;
#[cfg(test)]
pub mod mock;
pub mod health;
pub mod protocol;
pub mod result;
pub mod types;

#[cfg(test)]
mod ws_tests;
#[cfg(test)]
mod p3_tests;

pub use adapter::{Checker, MiaoSpeedAdapter};
pub use health::{cross_tag_alive, health_from_result, should_overwrite, snapshot_from_result, NodeHealth, PerTagHealth};
pub use protocol::{build_request_body, finalize_request, hash_miao_speed, sign_request};
pub use result::{normalize, reconcile, NormalizeContext, NormalizedCheckResult};
pub use types::{
    CheckJobId, CheckResult, CheckerError, CheckerStatus, DeepResult, InitialResult, SubmitRequest,
    VendorNode,
};
