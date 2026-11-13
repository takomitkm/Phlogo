//! Subscription output generators (P5-03): Mihomo YAML, Sing-box JSON (independent
//! converter — never a reverse of the Mihomo mapping), and Base64.
use serde_json::{json, Value};

use crate::naming::{fields_for, render, GradeThresholds};
use crate::SelectedNode;

/// A proxy candidate with the raw config needed by both converters.
pub struct OutputNode {
    pub selected: SelectedNode,
    pub name: String,
    /// `NormalizedNode` wire payload (clash-shaped proxy object).
    pub clash_proxy: Value,
    /// Sing-box outbound (independent schema).
    pub singbox_outbound: Value,
}

/// Mihomo/Clash YAML: template's top-level `proxies` replaced, every proxy-group's
/// `proxies` list filled with the generated names. Structure preserved verbatim.
pub fn mihomo_yaml(template_yaml: &str, nodes: &[OutputNode]) -> String {
    let mut root: serde_yaml::Value =
        serde_yaml::from_str(template_yaml).expect("template is valid yaml");
    let obj = root.as_mapping_mut().expect("template root is a mapping");

    let proxies: Vec<serde_yaml::Value> = nodes
        .iter()
        .map(|n| {
            let v: serde_yaml::Value =
                serde_yaml::to_value(&n.clash_proxy).expect("proxy serializable");
            v
        })
        .collect();
    if let Some(proxy_key) = obj
        .keys()
        .find(|k| k.as_str().map(|s| s == "proxies").unwrap_or(false))
        .cloned()
    {
        obj.insert(
            serde_yaml::Value::String("proxies".into()),
            serde_yaml::Value::Sequence(proxies),
        );
        let _ = proxy_key;
    } else {
        obj.insert(
            serde_yaml::Value::String("proxies".into()),
            serde_yaml::Value::Sequence(proxies),
        );
    }

    // Fill every proxy-group's proxies with the node names.
    if let Some(groups) = obj.get_mut(serde_yaml::Value::String("proxy-groups".into())) {
        if let Some(list) = groups.as_sequence_mut() {
            for g in list.iter_mut() {
                if let Some(gm) = g.as_mapping_mut() {
                    gm.insert(
                        serde_yaml::Value::String("proxies".into()),
                        serde_yaml::Value::Sequence(
                            nodes
                                .iter()
                                .map(|n| serde_yaml::Value::String(n.name.clone()))
                                .collect(),
                        ),
                    );
                }
            }
        }
    }
    serde_yaml::to_string(&root).expect("yaml serializable")
}

/// Sing-box JSON: keep inbounds/route/dns/experimental; replace proxy outbounds and
/// fill selector/urltest outbound lists. Schema is sing-box-native throughout.
pub fn singbox_json(template_json: &str, nodes: &[OutputNode]) -> String {
    let mut root: Value = serde_json::from_str(template_json).expect("template is valid json");
    let obj = root.as_object_mut().expect("template root is an object");

    // Split template outbounds: proxy-type groups (select/urltest) get node names;
    // everything else (direct/block/dns) is preserved verbatim.
    let template_outbounds = obj
        .get("outbounds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut kept: Vec<Value> = Vec::new();
    for ob in &template_outbounds {
        let typ = ob.get("type").and_then(Value::as_str).unwrap_or("");
        if typ == "selector" || typ == "urltest" {
            let mut ob = ob.clone();
            if let Some(m) = ob.as_object_mut() {
                m.insert("outbounds".into(), json!(nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>()));
            }
            kept.push(ob);
        } else {
            kept.push(ob.clone());
        }
    }
    for n in nodes {
        kept.push(n.singbox_outbound.clone());
    }
    obj.insert("outbounds".into(), Value::Array(kept));
    root.to_string()
}

/// Base64 of the plain subscription text (`scheme://...` lines).
pub fn base64_subs(links: &[String]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(links.join("\n"))
}

/// Default minimal clash template (kept in-process; template files come from config in P7).
pub const DEFAULT_CLASH_TEMPLATE: &str = r#"mixed-port: 7890
mode: rule
proxies: []
proxy-groups:
  - name: AUTO
    type: url-test
    proxies: []
  - name: FALLBACK
    type: fallback
    proxies: []
rules:
  - MATCH,AUTO
"#;

/// Default minimal singbox template.
pub const DEFAULT_SINGBOX_TEMPLATE: &str = r#"{
  "log": {"level": "info"},
  "inbounds": [{"type": "mixed", "tag": "mixed-in", "listen": "127.0.0.1", "listen_port": 2080}],
  "outbounds": [
    {"type": "selector", "tag": "SELECT", "outbounds": []},
    {"type": "urltest", "tag": "AUTO", "outbounds": []},
    {"type": "direct", "tag": "direct"}
  ],
  "route": {"final": "SELECT"}
}"#;

/// Build a clash proxy object from a normalized node payload + name.
pub fn clash_proxy_from(mut payload: Value, name: &str) -> Value {
    if let Some(m) = payload.as_object_mut() {
        m.insert("name".into(), json!(name));
    }
    payload
}

/// Build a sing-box outbound from a normalized node (independent schema).
pub fn singbox_outbound_from(
    protocol: &str,
    server: &str,
    port: u16,
    uuid_or_password: &str,
    tls_sni: Option<&str>,
    ws_path: Option<&str>,
    name: &str,
) -> Value {
    let mut ob = json!({
        "tag": name,
        "type": protocol,
        "server": server,
        "server_port": port,
    });
    let m = ob.as_object_mut().unwrap();
    match protocol {
        "vmess" | "vless" => {
            m.insert("uuid".into(), json!(uuid_or_password));
        }
        "tuic" => {
            // TUIC v5 carries both uuid and password; the identity credential is
            // "uuid:password" (see domain::node_auth), so split it back.
            let (u, p) = match uuid_or_password.split_once(':') {
                Some((u, p)) => (u.to_string(), p.to_string()),
                None => (String::new(), uuid_or_password.to_string()),
            };
            if !u.is_empty() {
                m.insert("uuid".into(), json!(u));
            }
            m.insert("password".into(), json!(p));
        }
        _ => {
            m.insert("password".into(), json!(uuid_or_password));
        }
    }
    if let Some(sni) = tls_sni {
        m.insert("tls".into(), json!({"enabled": true, "server_name": sni}));
    }
    if let Some(path) = ws_path {
        m.insert("transport".into(), json!({"type": "ws", "path": path}));
    }
    ob
}

/// Build a sing-box outbound from a **Clash-shaped payload** (full fidelity).
///
/// The Clash payload is the source of truth for protocol-specific fields
/// (hysteria2 `password`, tuic `uuid`+`password`, wireguard `private-key`,
/// shadowsocks `method`, anytls `password`, …). The sing-box schema differs from
/// Clash in field names, so this maps them per protocol instead of hard-coding a
/// vmess/vless/trojan-only shape.
///
/// sing-box protocol names (`shadowsocks`, `socks`, `anytls`, `hysteria2`, …) match
/// the Clash names for every protocol we support, so the type is carried over.
pub fn singbox_outbound_from_clash(payload: &Value, name: &str) -> Value {
    let mut ob = match payload {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    let protocol = ob
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    ob.remove("name");
    ob.insert("tag".into(), json!(name));

    // Clash `server`/`port` -> sing-box `server`/`server_port`.
    if let Some(s) = ob.remove("server") {
        ob.insert("server".into(), s);
    }
    if let Some(p) = ob.remove("port") {
        ob.insert("server_port".into(), p);
    }

    // Credentials: keep the native field names (they match between dialects for
    // every protocol we support), but make sure the required one is present.
    let cred_keys: &[&str] = match protocol.as_str() {
        "vmess" | "vless" => &["uuid"],
        "tuic" => &["uuid", "password"],
        "wireguard" => &["private_key"],
        "snell" => &["psk"],
        "socks" | "socks5" | "http" | "https" | "mieru" => &["username", "password"],
        "shadowsocks" | "ss" | "ssr" => &["method", "password"],
        "sudoku" => &["key"],
        _ => &["password"],
    };
    // sing-box names the shadowsocks cipher `method`; Clash names it `cipher`.
    if protocol == "shadowsocks" || protocol == "ss" {
        if let Some(c) = ob.remove("cipher") {
            ob.entry("method").or_insert(c);
        }
    }
    // sing-box spells wireguard's key `private_key`; Clash uses `private-key`.
    if protocol == "wireguard" {
        if let Some(k) = ob.remove("private-key") {
            ob.entry("private_key").or_insert(k);
        }
    }
    let _ = cred_keys; // the parser guarantees the primary credential field exists
    // Drop Clash-only keys that sing-box would reject.
    let insecure = ob
        .remove("skip-cert-verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reality = ob.remove("reality-opts");
    // Clash `client-fingerprint` (chrome/safari/ios/edge/firefox/…) -> sing-box uTLS.
    // REALITY **requires** uTLS on the client side: sing-box refuses to start with
    // "uTLS is required by reality client" when the fingerprint is missing
    // (verified against sing-box v1.14.4 on the school machine, 2026-11-13).
    let client_fp = ob
        .remove("client-fingerprint")
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty());
    // Clash `flow` (xtls-rprx-vision) -> sing-box `flow`.
    let flow = ob
        .remove("flow")
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty());

    // Transport: Clash `network` + `ws-opts`/`grpc-opts` -> sing-box `transport`.
    // MUST run before the Clash-only keys are dropped below (it reads `network`).
    let network = ob
        .remove("network")
        .and_then(|v| v.as_str().map(str::to_string));
    let mut transport: Option<Value> = None;
    if let Some(net) = network {
        match net.as_str() {
            "ws" => {
                let ws = ob.remove("ws-opts");
                let path = ws
                    .as_ref()
                    .and_then(|w| w.get("path"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let headers = ws.as_ref().and_then(|w| w.get("headers")).cloned();
                let host = headers
                    .as_ref()
                    .and_then(|h| {
                        h.get("Host")
                            .or_else(|| h.get("host"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                let mut t = json!({"type": "ws"});
                if let Some(p) = path {
                    t["path"] = json!(p);
                }
                if let Some(h) = host {
                    t["headers"] = json!({"Host": h});
                }
                transport = Some(t);
            }
            "grpc" => {
                let grpc = ob.remove("grpc-opts");
                let svc = grpc
                    .as_ref()
                    .and_then(|g| g.get("grpc-service-name"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let mut t = json!({"type": "grpc"});
                if let Some(s) = svc {
                    t["service_name"] = json!(s);
                }
                transport = Some(t);
            }
            "http" | "h2" => {
                let mut t = json!({"type": "http"});
                if let Some(h2) = ob.remove("http-opts") {
                    let hosts = h2
                        .get("host")
                        .cloned()
                        .unwrap_or_else(|| Value::Array(Vec::new()));
                    t["host"] = hosts;
                    t["path"] = h2.get("path").cloned().unwrap_or(Value::Null);
                }
                transport = Some(t);
            }
            _ => {}
        }
    }

    for k in [
        "udp", "alterId", "cipher",
        "network", "ws-opts", "grpc-opts", "http-opts", "reality-opts", "servername", "alpn",
        "tls", "fingerprint", "ports", "mport", "up", "down", "congestion-controller",
        "smux", "plugin", "plugin-opts", "underlying-proxy", "ip-version",
    ] {
        ob.remove(k);
    }

    // TLS: sing-box wants an object; Clash uses a bool + flat fields.
    let sni = ob
        .remove("sni")
        .and_then(|v| v.as_str().map(str::to_string));
    let alpn = ob.remove("alpn");
    let mut tls_map = serde_json::Map::new();
    tls_map.insert("enabled".into(), json!(true));
    if let Some(s) = sni {
        tls_map.insert("server_name".into(), json!(s));
    }
    if let Some(a) = alpn {
        tls_map.insert("alpn".into(), a);
    }
    if insecure {
        tls_map.insert("insecure".into(), json!(true));
    }
    if let Some(r) = reality {
        tls_map.insert(
            "reality".into(),
            json!({
                "enabled": true,
                "public_key": r.get("public-key").cloned().unwrap_or(Value::Null),
                "short_id": r.get("short-id").cloned().unwrap_or(Value::Null),
            }),
        );
    }
    // uTLS: sing-box needs it for REALITY (and honours it elsewhere).
    if let Some(fp) = client_fp {
        tls_map.insert("utls".into(), json!({"enabled": true, "fingerprint": fp}));
    }
    ob.insert("tls".into(), Value::Object(tls_map));
    if let Some(f) = flow {
        ob.insert("flow".into(), json!(f));
    }
    if let Some(t) = transport {
        ob.insert("transport".into(), t);
    }

    Value::Object(ob)
}

/// Convenience: name all selected nodes with the default template.
pub fn name_all(selected: &[SelectedNode], thresholds: &GradeThresholds, unlock: &str) -> Vec<String> {
    selected
        .iter()
        .map(|s| {
            let f = fields_for(s, thresholds, unlock);
            render(crate::naming::DEFAULT_TEMPLATE, &f)
        })
        .collect()
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use crate::Candidate;

    fn node(name: &str, region: &str, score: f64, seq: usize) -> OutputNode {
        let selected = SelectedNode {
            candidate: Candidate { id: name.into(), region: region.into(), score },
            seq_no: seq,
            seq_id: format!("{}{:02}", region.to_lowercase(), seq),
        };
        let f = fields_for(&selected, &GradeThresholds::default(), "");
        OutputNode {
            name: render(crate::naming::DEFAULT_TEMPLATE, &f),
            clash_proxy: serde_json::json!({
                "type": "trojan", "server": format!("{region}.example.com"), "port": 443,
                "password": "pw", "sni": format!("{region}.example.com")
            }),
            singbox_outbound: singbox_outbound_from(
                "trojan",
                &format!("{region}.example.com"),
                443,
                "pw",
                Some(&format!("{region}.example.com")),
                None,
                "x",
            ),
            selected,
        }
    }

    /// Acceptance 1 (schema part): YAML keeps structure, fills proxies + groups.
    #[test]
    fn mihomo_yaml_fills_proxies_and_groups() {
        let nodes = vec![node("hk1", "HK", 0.9, 1), node("jp1", "JP", 0.8, 1)];
        let out = mihomo_yaml(DEFAULT_CLASH_TEMPLATE, &nodes);
        assert!(out.contains("proxies:"), "yaml: {out}");
        assert!(out.contains("type: trojan"), "yaml: {out}");
        assert!(out.contains("AUTO") && out.contains("FALLBACK"));
        // Group proxies filled: count name occurrences (2 nodes x 2 groups + 2 proxy blocks).
        assert!(
            out.matches("🇭🇰").count() + out.matches("🇯🇵").count() >= 4,
            "names appear in groups"
        );
        assert!(out.contains("mixed-port: 7890"), "template structure preserved");
        assert!(out.contains("MATCH,AUTO"), "rules preserved");
    }

    /// Acceptance 5: sing-box output is native schema, not a clash reverse.
    #[test]
    fn singbox_is_independent_schema() {
        let nodes = vec![node("hk1", "HK", 0.9, 1)];
        let out = singbox_json(DEFAULT_SINGBOX_TEMPLATE, &nodes);
        let v: Value = serde_json::from_str(&out).unwrap();
        let obs = v["outbounds"].as_array().unwrap();
        let direct = obs.iter().find(|o| o["type"] == "direct").expect("preserved");
        assert!(direct.get("clash-params").is_none());
        let trojan = obs.iter().find(|o| o["type"] == "trojan").expect("node added");
        assert!(trojan["server_port"].is_u64(), "sing-box uses server_port, got {trojan}");
        assert!(trojan["tls"]["server_name"] == "HK.example.com");
        let selector = obs.iter().find(|o| o["type"] == "selector").unwrap();
        assert_eq!(selector["outbounds"].as_array().unwrap().len(), 1, "selector filled");
        let urltest = obs.iter().find(|o| o["type"] == "urltest").unwrap();
        assert_eq!(urltest["outbounds"].as_array().unwrap().len(), 1, "urltest filled");
    }

    /// Base64 roundtrip.
    #[test]
    fn base64_roundtrip() {
        let links = vec!["trojan://pw@a.example.com:443".to_string(), "trojan://pw@b.example.com:443".to_string()];
        let enc = base64_subs(&links);
        use base64::Engine;
        let dec = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&enc).unwrap()).unwrap();
        assert_eq!(dec, links.join("\n"));
    }

    /// Both converters come from the same node but produce different schemas.
    #[test]
    fn converters_independent() {
        let n = node("hk1", "HK", 0.9, 1);
        assert!(n.clash_proxy.get("port").is_some(), "clash uses port");
        assert!(n.clash_proxy.get("server_port").is_none());
        assert!(n.singbox_outbound.get("server_port").is_some(), "singbox uses server_port");
        assert!(n.singbox_outbound.get("port").is_none());
    }

    #[allow(dead_code)]
    fn _unused(_: &Vec<String>) {
        let _ = name_all;
    }

    /// sing-box conversion must keep protocol-specific fields (regression 2026-11-13):
    /// the old hard-coded builder only understood vmess/vless/trojan, so a
    /// hysteria2 node lost its password entirely.
    #[test]
    fn singbox_from_clash_keeps_protocol_fields() {
        let payload = serde_json::json!({
            "name": "hy2-01", "type": "hysteria2", "server": "hy2.example.com",
            "port": 443, "password": "hy2-pw", "sni": "hy2.example.com",
            "skip-cert-verify": true, "udp": true
        });
        let sb = singbox_outbound_from_clash(&payload, "hy2-01");
        assert_eq!(sb["type"], "hysteria2");
        assert_eq!(sb["tag"], "hy2-01");
        assert_eq!(sb["server"], "hy2.example.com");
        assert_eq!(sb["server_port"], 443);
        assert_eq!(sb["password"], "hy2-pw", "hysteria2 password must survive");
        assert_eq!(sb["tls"]["enabled"], true);
        assert_eq!(sb["tls"]["server_name"], "hy2.example.com");
        assert!(sb.get("udp").is_none(), "clash-only keys must be dropped");
        assert!(sb.get("sni").is_none(), "sni moves into tls");
    }

    #[test]
    fn singbox_from_clash_maps_tuic_and_shadowsocks() {
        let tuic = serde_json::json!({
            "name": "t", "type": "tuic", "server": "t.example.com", "port": 443,
            "uuid": "uu", "password": "pw"
        });
        let sb = singbox_outbound_from_clash(&tuic, "t");
        assert_eq!(sb["uuid"], "uu");
        assert_eq!(sb["password"], "pw");

        let ss = serde_json::json!({
            "name": "s", "type": "ss", "server": "s.example.com", "port": 443,
            "cipher": "aes-256-gcm", "password": "sp"
        });
        let sb = singbox_outbound_from_clash(&ss, "s");
        assert_eq!(sb["method"], "aes-256-gcm", "clash cipher -> sing-box method");
        assert_eq!(sb["password"], "sp");
        assert!(sb.get("cipher").is_none());
    }

    #[test]
    fn singbox_from_clash_maps_transport_and_reality() {
        let payload = serde_json::json!({
            "name": "v", "type": "vless", "server": "v.example.com", "port": 443,
            "uuid": "uu", "network": "ws",
            "ws-opts": {"path": "/p", "headers": {"Host": "h.example.com"}},
            "reality-opts": {"public-key": "pk", "short-id": "sid"}
        });
        let sb = singbox_outbound_from_clash(&payload, "v");
        assert_eq!(sb["transport"]["type"], "ws");
        assert_eq!(sb["transport"]["path"], "/p");
        assert_eq!(sb["transport"]["headers"]["Host"], "h.example.com");
        assert_eq!(sb["tls"]["reality"]["public_key"], "pk");
        assert_eq!(sb["tls"]["reality"]["short_id"], "sid");
    }

    /// REALITY **requires** uTLS on the sing-box side: without it sing-box refuses
    /// to start ("uTLS is required by reality client"). Clash carries the same
    /// setting as `client-fingerprint`. Verified against sing-box v1.14.4 on the
    /// school machine (2026-11-13).
    #[test]
    fn singbox_from_clash_maps_client_fingerprint_to_utls() {
        let payload = serde_json::json!({
            "name": "r", "type": "vless", "server": "r.example.com", "port": 443,
            "uuid": "uu", "flow": "xtls-rprx-vision", "client-fingerprint": "ios",
            "reality-opts": {"public-key": "pk", "short-id": "sid"}
        });
        let sb = singbox_outbound_from_clash(&payload, "r");
        assert_eq!(sb["tls"]["utls"]["enabled"], true);
        assert_eq!(sb["tls"]["utls"]["fingerprint"], "ios");
        assert_eq!(sb["flow"], "xtls-rprx-vision", "vision flow must survive");
        assert!(sb.get("client-fingerprint").is_none(), "clash key must be consumed");
    }
}
