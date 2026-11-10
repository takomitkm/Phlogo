//! Minimal MiaoSpeed mock (per P0-02 frozen protocol, tag 4.7.9) for adapter unit tests.
//! Contract: WS text frames; each request is one authed `SlaveRequest`; the mock verifies
//! the Challenge the same way the server does (re-serialize with Vendor="" and compare),
//! replies `{"Error":...}` on failure, and streams Progress frames + a terminal Result frame.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{build_request_body, hash_miao_speed};

#[derive(Clone)]
pub struct MockResult {
    pub vendor_id: String,
    pub latency_ms: i64,
}

pub struct MockMiaoSpeed {
    pub addr: String,
    pub token: String,
}

impl MockMiaoSpeed {
    /// Start the mock; `results` is the canned result set returned to any valid request.
    pub async fn start(token: &str, results: Vec<MockResult>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr: SocketAddr = listener.local_addr().unwrap();
        let token_owned = token.to_string();

        tokio::spawn(async move {
            let token = token_owned;
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let token = token.clone();
                let results = results.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, token, results).await;
                });
            }
        });

        Self {
            addr: format!("ws://{addr}"),
            token: token.to_string(),
        }
    }
}

fn verify(req: &Value, token: &str) -> bool {
    let claimed = req["Challenge"].as_str().unwrap_or_default().to_string();
    let body = build_request_body(req, ""); // server signs the Vendor="" clone (D.5)
    hash_miao_speed(token, body.as_bytes()) == claimed
}

async fn serve(stream: TcpStream, token: String, results: Vec<MockResult>) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut tx, mut rx) = ws.split();

    while let Some(Ok(msg)) = rx.next().await {
        let Message::Text(t) = msg else {
            continue;
        };
        let Ok(req) = serde_json::from_str::<Value>(&t) else {
            continue;
        };
        if !verify(&req, &token) {
            tx.send(Message::Text(
                json!({"Error": "cannot verify the request, please check your token"})
                    .to_string(),
            ))
            .await
            .ok();
            return;
        }
        let task_id = uuid::Uuid::new_v4().to_string();
        // initial queuing progress
        tx.send(Message::Text(
            json!({"ID": task_id, "MiaoSpeedVersion": "4.7.9", "Error": "", "Result": null,
                   "Progress": {"Index": 0, "Record": null, "Queuing": results.len()}})
                .to_string(),
        ))
        .await
        .ok();
        // per-node progress
        for (i, r) in results.iter().enumerate() {
            tx.send(Message::Text(
                json!({"ID": task_id, "MiaoSpeedVersion": "4.7.9", "Error": "", "Result": null,
                       "Progress": {"Index": i, "Queuing": 0, "Record": {
                           "Grouping": "", "InvokeDuration": r.latency_ms,
                           "ProxyInfo": {"Name": r.vendor_id, "Address": "127.0.0.1", "Type": "Invalid"},
                           "Matrices": [{"Type": "TEST_PING_RTT", "Payload": "{\"Value\":0}"}]}}})
                    .to_string(),
            ))
            .await
            .ok();
        }
        // terminal Result frame
        let slots: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({"Grouping": "", "InvokeDuration": r.latency_ms,
                       "ProxyInfo": {"Name": r.vendor_id, "Address": "127.0.0.1", "Type": "Invalid"},
                       "Matrices": [{"Type": "TEST_PING_RTT", "Payload": "{\"Value\":0}"}]})
            })
            .collect();
        tx.send(Message::Text(
            json!({"ID": task_id, "MiaoSpeedVersion": "4.7.9", "Error": "",
                   "Result": {"Request": req, "Results": slots}, "Progress": null})
                .to_string(),
        ))
        .await
        .ok();
    }
}
