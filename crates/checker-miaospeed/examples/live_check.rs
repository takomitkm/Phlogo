//! Drive a REAL MiaoSpeed server through the adapter (M2 verification).
//!
//! Usage: live_check <ws-url> <secret> <vendor> <clash-payload-json>...
//!
//! Vendors: `Clash` (payload = one Clash proxy object, what we store) or `Local`.
//! Prints one line per mapped result; the exit code is 0 only when the server answered
//! with a terminal Result frame.
use checker_miaospeed::{MiaoSpeedAdapter, SubmitRequest, VendorNode};
use serde_json::Value;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: live_check <ws-url> <secret> <vendor> <payload>..."))?;
    let secret = args.next().unwrap_or_default();
    let vendor = args.next().unwrap_or_else(|| "Clash".to_string());
    let payloads: Vec<String> = args.collect();
    if payloads.is_empty() {
        return Err(anyhow::anyhow!("need at least one payload"));
    }

    let nodes: Vec<VendorNode> = payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let payload: Value = serde_json::from_str(p)
                .unwrap_or_else(|_| Value::String(p.clone()));
            VendorNode {
                internal_node_id: Uuid::new_v4(),
                vendor_id: format!("phlogo-live-{i}"),
                payload,
            }
        })
        .collect();

    let request = SubmitRequest {
        run_id: Uuid::new_v4(),
        checker_tag: "live".into(),
        vendor: vendor.clone(),
        presets: vec!["TEST_PING_RTT".into()],
        nodes,
    };

    let adapter = MiaoSpeedAdapter::new(url.clone(), secret);
    let results = adapter.check(&request).await?;
    println!("vendor={vendor} url={url} results={}", results.len());
    for r in &results {
        println!(
            "node={} latency_ms={} deep={}",
            r.internal_node_id,
            r.initial.latency_ms,
            r.deep.is_some()
        );
    }
    if results.is_empty() {
        return Err(anyhow::anyhow!("server returned no results"));
    }
    Ok(())
}