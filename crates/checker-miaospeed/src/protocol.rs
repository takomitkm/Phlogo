//! MiaoSpeed wire protocol, frozen by P0-02 (tag 4.7.9).
//! Serialization must be byte-identical to Go jsoniter re-marshaling: Go field names,
//! declaration order, inlined `Configs`, zero values always emitted, empty slices as `[]`,
//! and no HTML escaping (jsoniter default). Auth is per-request inline: the `Challenge`
//! field carries `hashMiaoSpeed(token, json(req with Challenge=""))`.
//!
//! The `Clone()` quirk (D.5): the server signs a Clone of the decoded request that DROPS
//! `Vendor`, so the client must compute the Challenge over a `Vendor:""` body while
//! sending the real vendor string.
use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha512};

pub const BUILDTOKEN_KEY: &str = "MIAOKO4|580JxAo049R|GEnERAl|1X571R930|T0kEN";

/// `hashMiaoSpeed` (D.4): chained SHA512 over request bytes + build tokens.
pub fn hash_miao_speed(token: &str, request_bytes: &[u8]) -> String {
    let mut tokens: Vec<&str> = vec![token];
    tokens.extend(BUILDTOKEN_KEY.split('|'));
    let mut h = Sha512::new();
    h.update(request_bytes);
    for t in tokens {
        let t = if t.is_empty() { "SOME_TOKEN" } else { t };
        let digest = h.clone().finalize();
        h.update(t.as_bytes());
        h.update(digest);
    }
    base64::engine::general_purpose::URL_SAFE.encode(h.finalize())
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn i64_of(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

/// Build the jsoniter-shaped `SlaveRequest` JSON (D.3). Field order is load-bearing:
/// the server re-serializes the decoded struct in Go declaration order and compares
/// signatures over those bytes.
pub fn build_request_body(req: &Value, vendor_for_signing: &str) -> String {
    let basics = &req["Basics"];
    let options = &req["Options"];
    let configs = &req["Configs"];
    let nodes = req["Nodes"].as_array().cloned().unwrap_or_default();

    let basics_obj = json!({
        "ID": s(&basics["ID"]),
        "Slave": s(&basics["Slave"]),
        "SlaveName": s(&basics["SlaveName"]),
        "Invoker": s(&basics["Invoker"]),
        "Version": s(&basics["Version"]),
    });
    let matrices: Vec<Value> = options["Matrices"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            json!({
                "Type": s(&m["Type"]),
                "Params": s(&m["Params"]),
            })
        })
        .collect();
    let configs_obj = json!({
        "STUNURL": s(&configs["STUNURL"]),
        "DownloadURL": s(&configs["DownloadURL"]),
        "DownloadDuration": i64_of(&configs["DownloadDuration"]),
        "DownloadThreading": i64_of(&configs["DownloadThreading"]),
        "PingAverageOver": i64_of(&configs["PingAverageOver"]),
        "PingAddress": s(&configs["PingAddress"]),
        "TaskRetry": i64_of(&configs["TaskRetry"]),
        "DNSServers": configs["DNSServers"].as_array().cloned().unwrap_or_else(|| vec![json!("")]).iter().map(|x| json!(s(x))).collect::<Vec<_>>(),
        "TaskTimeout": i64_of(&configs["TaskTimeout"]),
        "Scripts": configs["Scripts"].as_array().cloned().unwrap_or_default(),
        "ApiVersion": i64_of(&configs["ApiVersion"]),
        "UploadURL": s(&configs["UploadURL"]),
        "UploadDuration": i64_of(&configs["UploadDuration"]),
        "UploadThreading": i64_of(&configs["UploadThreading"]),
    });
    let nodes_arr: Vec<Value> = nodes
        .iter()
        .map(|n| json!({"Name": s(&n["Name"]), "Payload": s(&n["Payload"])}))
        .collect();

    // serde_json objects preserve insertion order (feature "preserve_order" is NOT enabled,
    // BTreeMap sorts keys). jsoniter output order = Go struct declaration order, which for
    // SlaveRequest IS alphabetical as observed in P0-02's captured body, so object maps are safe.
    let mut root = Map::new();
    root.insert("Basics".into(), basics_obj);
    root.insert("Options".into(), json!({"Filter": s(&options["Filter"]), "Matrices": matrices}));
    root.insert("Configs".into(), configs_obj);
    root.insert("Vendor".into(), json!(vendor_for_signing));
    root.insert("Nodes".into(), json!(nodes_arr));
    root.insert("RandomSequence".into(), json!(s(&req["RandomSequence"])));
    root.insert("Challenge".into(), json!(""));
    Value::Object(root).to_string()
}

/// Compute the Challenge for a request (signs the `Vendor:""` clone per D.5).
pub fn sign_request(token: &str, req: &Value) -> String {
    hash_miao_speed(token, build_request_body(req, "").as_bytes())
}

/// Final wire body: real vendor + Challenge filled in.
pub fn finalize_request(token: &str, req: &Value, vendor: &str) -> String {
    let mut body = build_request_body(req, vendor);
    // Replace the trailing "Challenge":"" with the signed value by re-serializing with it set.
    let mut root: Value = serde_json::from_str(&body).expect("valid json");
    root["Challenge"] = json!(sign_request(token, req));
    body = root.to_string();
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn golden_signature_matches_p0_02() {
        // Captured in P0-02 (E): Go utils.SignRequest("SPIKETOKEN", &req) for this body.
        let req = json!({
            "Basics": {"ID":"spike-0001","Slave":"","SlaveName":"phlogo-spike","Invoker":"phlogo","Version":"0.1.0"},
            "Options": {"Filter":"","Matrices":[{"Type":"TEST_PING_RTT","Params":""},{"Type":"TEST_PING_CONN","Params":""}]},
            "Configs": {"STUNURL":"","DownloadURL":"","DownloadDuration":3,"DownloadThreading":1,"PingAverageOver":1,
                "PingAddress":"http://gstatic.com/generate_204","TaskRetry":3,"DNSServers":[],"TaskTimeout":5000,"Scripts":[],
                "ApiVersion":3,"UploadURL":"","UploadDuration":0,"UploadThreading":0},
            "Vendor":"Local","Nodes":[{"Name":"direct-gstatic","Payload":""}],"RandomSequence":"","Challenge":""}
        );
        let sig = sign_request("SPIKETOKEN", &req);
        assert_eq!(
            sig,
            "fKdMhefuCZz2RmiaQxar5SLkAMnLd5i7D1_ZUYSHCpQykOT_smWDJ-pnhmTgSyOnJl7c97mR90bw6UIe4hMJ9Q==",
            "must match the P0-02 Go cross-validated signature byte for byte"
        );
    }

    #[test]
    fn vendor_quirk_signature_differs() {
        let req = json!({
            "Basics": {"ID":"x","Slave":"","SlaveName":"n","Invoker":"i","Version":"v"},
            "Options": {"Filter":"","Matrices":[]},
            "Configs": {"STUNURL":"","DownloadURL":"","DownloadDuration":0,"DownloadThreading":0,"PingAverageOver":0,
                "PingAddress":"","TaskRetry":0,"DNSServers":[],"TaskTimeout":0,"Scripts":[],
                "ApiVersion":3,"UploadURL":"","UploadDuration":0,"UploadThreading":0},
            "Vendor":"Local","Nodes":[],"RandomSequence":"","Challenge":""}
        );
        let with_vendor = hash_miao_speed("t", build_request_body(&req, "Local").as_bytes());
        let without = sign_request("t", &req);
        assert_ne!(with_vendor, without, "vendor must be empty in the signed clone");
    }

    #[test]
    fn url_safe_base64_with_padding() {
        let out = hash_miao_speed("t", b"x");
        assert!(out.ends_with('=') || out.len().is_multiple_of(4));
        assert!(!out.contains('+') && !out.contains('/'));
        let _ = base64::engine::general_purpose::URL_SAFE.decode(&out).unwrap();
    }
}
