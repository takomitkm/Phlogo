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
}
