//! MiaoSpeed WebSocket adapter (P3-02). Implements the plan §8 `Checker` trait over the
//! protocol frozen by P0-02 (tag 4.7.9): inline per-request auth via `hashMiaoSpeed`,
//! the `Clone()` Vendor-drop quirk, byte-exact jsoniter-shaped serialization, and
//! `ProxyInfo.Name`-based result mapping.
pub mod adapter;
#[cfg(test)]
pub mod mock;
pub mod protocol;
pub mod types;

#[cfg(test)]
mod ws_tests;

pub use adapter::{Checker, MiaoSpeedAdapter};
pub use protocol::{build_request_body, finalize_request, hash_miao_speed, sign_request};
pub use types::{
    CheckJobId, CheckResult, CheckerError, CheckerStatus, DeepResult, InitialResult, SubmitRequest,
    VendorNode,
};
