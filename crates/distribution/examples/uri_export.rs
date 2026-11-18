//! M1c validation helper: turn a real Clash/sing-box subscription file into the same
//! base64 share-link list the API would serve, so a real client binary can parse it.
//!
//! Usage: cargo run -p distribution --example uri_export -- <subscription-file>
//! Prints the base64 body on stdout; node/link counts go to stderr.
use uuid::Uuid;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: uri_export <subscription-file>"))?;
    let raw = std::fs::read(&path)?;
    let ctx = domain::SourceContext {
        source_id: Uuid::nil(),
        fetch_run_id: Uuid::nil(),
    };
    let nodes = vproxy_adapter::parse_subscription(&raw, &ctx)
        .map_err(|e| anyhow::anyhow!("parse {path}: {e}"))?;

    let mut links = Vec::new();
    let mut skipped = 0usize;
    for node in &nodes {
        let payload: serde_json::Value = serde_json::from_str(&node.raw_config_reference)?;
        match distribution::uri::uri_from_clash(&payload) {
            Some(uri) => links.push(uri),
            None => skipped += 1,
        }
    }
    eprintln!(
        "nodes={} links={} skipped={}",
        nodes.len(),
        links.len(),
        skipped
    );
    println!("{}", distribution::output::base64_subs(&links));
    Ok(())
}
