//! `MiaoSpeedAdapter`: `Checker` trait implementation over WebSocket (P0-02 protocol).
use std::collections::HashMap;
use std::sync::Mutex;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use crate::protocol::{finalize_request, sign_request};
use crate::types::{
    CheckJobId, CheckResult, CheckerError, CheckerStatus, InitialResult, SubmitRequest,
};

/// Plan §8 `Checker` abstraction (object-safe form).
#[allow(async_fn_in_trait)]
pub trait Checker {
    async fn submit(&self, job: SubmitRequest) -> Result<CheckJobId, CheckerError>;
    async fn collect(&self, job_id: &CheckJobId) -> Result<Vec<CheckResult>, CheckerError>;
    async fn status(&self) -> Result<CheckerStatus, CheckerError>;
}

pub struct MiaoSpeedAdapter {
    ws_url: String,
    /// Auth token; never logged (CONVENTIONS §2.11 #8).
    secret: String,
    /// vendor_id -> internal node id (session-scoped; see p3-03).
    id_map: Mutex<HashMap<String, Uuid>>,
    invoker: String,
}

impl MiaoSpeedAdapter {
    pub fn new(ws_url: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            ws_url: ws_url.into(),
            secret: secret.into(),
            id_map: Mutex::new(HashMap::new()),
            invoker: "phlogo".into(),
        }
    }

    /// Connect and send one authed request; returns the raw server responses.
    async fn roundtrip(&self, req: &Value, vendor: &str) -> Result<Vec<Value>, CheckerError> {
        let (mut ws, _) = connect_async(&self.ws_url)
            .await
            .map_err(|e| CheckerError::Connect(e.to_string()))?;

        let body = finalize_request(&self.secret, req, vendor);
        ws.send(Message::Text(body))
            .await
            .map_err(|e| CheckerError::Ws(e.to_string()))?;

        let mut out = Vec::new();
        // The terminal `Result` frame is the ONLY proof the task actually ran (M2.1):
        // an early Close without it must surface as a protocol error so the run lands
        // in the retry path instead of being persisted as a successful all-unknown check.
        let mut saw_terminal = false;
        while let Some(msg) = ws.next().await {
            let msg = msg.map_err(|e| CheckerError::Ws(e.to_string()))?;
            match msg {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t)
                        .map_err(|e| CheckerError::Protocol(e.to_string()))?;
                    debug_frame(&v);
                    if let Some(err) = v["Error"].as_str() {
                        if !err.is_empty() {
                            return Err(CheckerError::Auth(err.to_string()));
                        }
                    }
                    let done = v["Result"].is_object();
                    out.push(v);
                    if done {
                        saw_terminal = true;
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        if !saw_terminal {
            return Err(CheckerError::Protocol(
                "connection closed before the terminal Result frame".into(),
            ));
        }
        Ok(out)
    }

    fn new_request(&self, id: &str, matrices: &[String], nodes: &[Value]) -> Value {
        json!({
            "Basics": {"ID": id, "Slave": "", "SlaveName": "phlogo-gnosis-worker",
                        "Invoker": self.invoker, "Version": env!("CARGO_PKG_VERSION")},
            "Options": {"Filter": "", "Matrices": matrices
                .iter().map(|m| json!({"Type": m, "Params": ""})).collect::<Vec<_>>()},
            "Configs": {"STUNURL": "", "DownloadURL": "", "DownloadDuration": 0,
                "DownloadThreading": 0, "PingAverageOver": 1,
                "PingAddress": "http://gstatic.com/generate_204", "TaskRetry": 3,
                "DNSServers": [], "TaskTimeout": 5000, "Scripts": [],
                "ApiVersion": 3, "UploadURL": "", "UploadDuration": 0, "UploadThreading": 0},
            "Vendor": "Local",
            "Nodes": nodes,
            "RandomSequence": "",
            "Challenge": "",
        })
    }

    /// Build the wire node list.
    ///
    /// With vendor `Clash` MiaoSpeed reports results keyed by the **name inside the
    /// payload** (mihomo proxy name), not by `Nodes[].Name` — verified against the real
    /// 4.7.9 server on 2026-11-19, where a payload without `name` came back with an
    /// empty `ProxyInfo`. So the vendor id is injected into the payload before sending.
    fn wire_nodes(&self, nodes: &[crate::types::VendorNode]) -> Vec<Value> {
        nodes
            .iter()
            .map(|n| {
                let payload = match &n.payload {
                    Value::Object(map) => {
                        let mut m = map.clone();
                        m.insert("name".into(), json!(n.vendor_id));
                        // The wire schema wants the payload as a STRING (the server
                        // echoes it back); sending an object serializes to "".
                        Value::Object(m).to_string()
                    }
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                json!({"Name": n.vendor_id, "Payload": payload})
            })
            .collect()
    }

    fn map_results(&self, frames: &[Value]) -> Vec<CheckResult> {
        let map = self.id_map.lock().expect("id_map poisoned");
        let mut results = Vec::new();
        for frame in frames {
            let Some(slot) = frame["Result"]["Results"].as_array() else {
                continue;
            };
            for entry in slot {
                let name = entry["ProxyInfo"]["Name"].as_str().unwrap_or_default();
                let Some(internal) = map.get(name) else {
                    continue; // unknown vendor id (p3-03 handles late/dupes)
                };
                // Phlogo convention: -1 = dead. The real server reports 0 when it
                // could not even build/connect the proxy (verified against 4.7.9).
                let raw = entry["InvokeDuration"].as_i64().unwrap_or(0);
                let latency = if raw <= 0 { -1 } else { raw as i32 };
                results.push(CheckResult {
                    internal_node_id: *internal,
                    initial: InitialResult {
                        latency_ms: latency,
                    },
                    deep: None,
                });
            }
        }
        results
    }
}

impl Checker for MiaoSpeedAdapter {
    async fn submit(&self, job: SubmitRequest) -> Result<CheckJobId, CheckerError> {
        let task_id = Uuid::new_v4().to_string();
        let nodes = self.wire_nodes(&job.nodes);
        {
            let mut map = self.id_map.lock().expect("id_map poisoned");
            map.clear();
            for n in &job.nodes {
                map.insert(n.vendor_id.clone(), n.internal_node_id);
            }
        }
        let req = self.new_request(&task_id, &job.presets, &nodes);
        let _ = self.roundtrip(&req, &job.vendor).await?;
        Ok(CheckJobId(task_id))
    }

    /// MiaoSpeed returns progress + final result on the SAME request connection, so
    /// `collect` re-issues the stored task and reads the terminal `Result` frame.
    async fn collect(&self, job_id: &CheckJobId) -> Result<Vec<CheckResult>, CheckerError> {
        let req = self.new_request(&job_id.0, &["TEST_PING_RTT".into()], &[]);
        let frames = self.roundtrip(&req, "Local").await?;
        Ok(self.map_results(&frames))
    }

    async fn status(&self) -> Result<CheckerStatus, CheckerError> {
        // Inline auth doubles as a health probe: a signed roundtrip that does not
        // return an auth error proves the endpoint is alive and the token is valid.
        let req = self.new_request(&Uuid::new_v4().to_string(), &[], &[]);
        match self.roundtrip(&req, "Local").await {
            Ok(_) => Ok(CheckerStatus { online: true }),
            Err(CheckerError::Auth(e)) => Err(CheckerError::Auth(e)),
            Err(_) => Ok(CheckerStatus { online: false }),
        }
    }
}

impl MiaoSpeedAdapter {
    /// Run one task to completion in a single roundtrip and return mapped results (M2).
    ///
    /// `submit` + `collect` cannot drive a real server: MiaoSpeed reports progress and
    /// the terminal `Result` on the SAME request socket, so a second request carrying
    /// the same task id has no results to replay. This keeps one roundtrip and maps the
    /// terminal frame directly.
    pub async fn check(&self, job: &SubmitRequest) -> Result<Vec<CheckResult>, CheckerError> {
        let task_id = Uuid::new_v4().to_string();
        let nodes = self.wire_nodes(&job.nodes);
        {
            let mut map = self.id_map.lock().expect("id_map poisoned");
            map.clear();
            for n in &job.nodes {
                map.insert(n.vendor_id.clone(), n.internal_node_id);
            }
        }
        let req = self.new_request(&task_id, &job.presets, &nodes);
        let frames = self.roundtrip(&req, &job.vendor).await?;
        Ok(self.map_results(&frames))
    }
}

/// Structured, REDACTED frame dump for `PHLOGO_DEBUG_FRAMES` (M2.1 hardening).
///
/// The raw frame contains the full Clash payload of every node (passwords, UUIDs,
/// PSKs, private keys), so printing it verbatim would leak node credentials into the
/// log. Instead: protocol state, per-node Name/InvokeDuration (no Payload), and the
/// error text — with a recursion cap so a malformed frame can't blow up the log.
fn debug_frame(v: &Value) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static COUNT: AtomicU8 = AtomicU8::new(0);
    if COUNT.load(Ordering::Relaxed) >= 40 {
        return; // cap the debug output volume
    }
    COUNT.fetch_add(1, Ordering::Relaxed);

    let err = v["Error"].as_str().unwrap_or("");
    let has_result = v["Result"].is_object();
    let progress = &v["Progress"];
    let queueing = progress["Queuing"].as_i64();
    let idx = progress["Index"].as_i64();

    let mut line = format!(
        "frame: id={} terminal={} error={}",
        v["ID"].as_str().unwrap_or("?"),
        has_result,
        if err.is_empty() { "-" } else { err }
    );
    if let Some(q) = queueing {
        line.push_str(&format!(" queuing={q}"));
    }
    if let Some(i) = idx {
        line.push_str(&format!(" index={i}"));
    }
    // Progress record: name + latency only. NEVER the Payload.
    if let Some(rec) = &progress["Record"].as_object() {
        let name = rec
            .get("ProxyInfo")
            .and_then(|p| p.get("Name"))
            .and_then(|n| n.as_str())
            .unwrap_or("");
        let dur = rec
            .get("InvokeDuration")
            .and_then(|d| d.as_i64())
            .unwrap_or(0);
        if !name.is_empty() {
            line.push_str(&format!(" record={name} duration={dur}"));
        }
    }
    if has_result {
        let results = v["Result"]["Results"].as_array().map(|a| a.len());
        if let Some(n) = results {
            line.push_str(&format!(" results={n}"));
        }
    }
    eprintln!("{line}");
}

/// Unused import guard: `sign_request` is re-exported for contract tests.
#[allow(unused_imports)]
use sign_request as _sign_request;
