//! Core-side check types (plan §8 / p3-02 Step 2). Not MiaoSpeed-native shapes.
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum CheckerError {
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("websocket: {0}")]
    Ws(String),
    #[error("auth rejected: {0}")]
    Auth(String),
    #[error("protocol violation: {0}")]
    Protocol(String),
}

/// Adapter task handle returned by `submit`; used by `collect`.
#[derive(Debug, Clone)]
pub struct CheckJobId(pub String);

/// One submitted task (pure data from p3-01's `CheckJob`).
pub struct SubmitRequest {
    /// `check_runs.id`; the idempotency key.
    pub run_id: Uuid,
    pub checker_tag: String,
    /// MiaoSpeed vendor: "Local" (direct) or "Clash" (payload = mihomo proxy config).
    pub vendor: String,
    /// Matrix names passed through as config (whitelist per P0-02 D.7; never hardcoded here).
    pub presets: Vec<String>,
    /// Vendor nodes + the internal node_id each maps back to.
    pub nodes: Vec<VendorNode>,
}

pub struct VendorNode {
    pub internal_node_id: Uuid,
    /// Submitted as `Nodes[].Name`; results map back via `ProxyInfo.Name`.
    pub vendor_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub internal_node_id: Uuid,
    /// `InvokeDuration` ms; -1 = dead (Phlogo convention). Unit normalization lands in p3-03.
    pub initial: InitialResult,
    pub deep: Option<DeepResult>,
}

#[derive(Debug, Clone)]
pub struct InitialResult {
    pub latency_ms: i32,
}

#[derive(Debug, Clone)]
pub struct DeepResult {
    pub avg_latency_ms: i32,
    pub jitter_ms: i32,
    pub speed_kbps: i32,
    pub region: Option<String>,
    pub streaming: Value,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct CheckerStatus {
    pub online: bool,
}
