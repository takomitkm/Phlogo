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
        while let Some(msg) = ws.next().await {
            let msg = msg.map_err(|e| CheckerError::Ws(e.to_string()))?;
            match msg {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t)
                        .map_err(|e| CheckerError::Protocol(e.to_string()))?;
                    if let Some(err) = v["Error"].as_str() {
                        if !err.is_empty() {
                            return Err(CheckerError::Auth(err.to_string()));
                        }
                    }
                    let done = v["Result"].is_object();
                    out.push(v);
                    if done {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok(out)
    }

    fn new_request(&self, id: &str, matrices: &[String], nodes: &[Value]) -> Value {
        json!({
            "Basics": {"ID": id, "Slave": "", "SlaveName": "phlogo-worker",
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
                let latency = entry["InvokeDuration"].as_i64().unwrap_or(-1) as i32;
                results.push(CheckResult {
                    internal_node_id: *internal,
                    initial: InitialResult { latency_ms: latency },
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
        let nodes: Vec<Value> = job
            .nodes
            .iter()
            .map(|n| {
                json!({"Name": n.vendor_id,
                        "Payload": if n.payload.is_null() { Value::String(String::new()) } else { Value::String(n.payload.to_string()) }})
            })
            .collect();
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

/// Unused import guard: `sign_request` is re-exported for contract tests.
#[allow(unused_imports)]
use sign_request as _sign_request;
