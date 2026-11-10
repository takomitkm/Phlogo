//! VProxy Parser Adapter (minimal port; VProxy is NOT a dependency — P0-01 decision).
//! Parses Clash YAML / Sing-box JSON / Base64 subscriptions into `domain::NormalizedNode`,
//! keeping VProxy-like node types out of the DB layer (only domain types cross the boundary).
use domain::{Credential, NormalizedNode, SourceContext, TlsSettings, Transport};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("unrecognized subscription format")]
    UnrecognizedFormat,
    #[error("no supported nodes (all entries skipped)")]
    NoSupportedNodes,
}

fn get_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

fn get_u16(v: &Value, key: &str) -> Option<u16> {
    v.get(key).and_then(|x| x.as_u64()).map(|n| n as u16)
}

fn alpn_list(v: &Value) -> Vec<String> {
    v.get("alpn")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn cred_for(protocol: &str, obj: &Value) -> Option<Credential> {
    match protocol {
        "vmess" | "vless" => get_str(obj, "uuid").map(|s| Credential::Uuid(s.to_string())),
        "trojan" | "ss" | "shadowsocks" => get_str(obj, "password")
            .or_else(|| get_str(obj, "cipher"))
            .map(|s| Credential::Password(s.to_string())),
        _ => None,
    }
}

fn from_clash(p: &Value, ctx: &SourceContext) -> Option<NormalizedNode> {
    let protocol = get_str(p, "type")?.to_lowercase();
    let server = get_str(p, "server")?.to_string();
    let port = get_u16(p, "port")?;
    let credentials = cred_for(&protocol, p)?;

    let transport = get_str(p, "network").map(|kind| {
        let mut t = Transport {
            kind: kind.to_string(),
            ..Default::default()
        };
        if kind == "ws" {
            if let Some(ws) = p.get("ws-opts") {
                t.path = get_str(ws, "path").map(str::to_string);
                if let Some(h) = ws.get("headers") {
                    t.host = get_str(h, "Host")
                        .or_else(|| get_str(h, "host"))
                        .map(str::to_string);
                }
            }
        } else if kind == "grpc" {
            if let Some(grpc) = p.get("grpc-opts") {
                t.service_name = get_str(grpc, "grpc-service-name").map(str::to_string);
            }
        }
        t
    });

    let tls_flag = p.get("tls").and_then(|v| v.as_bool()).unwrap_or(false);
    let sni = get_str(p, "sni").map(str::to_string);
    let alpn = alpn_list(p);
    let reality = p
        .get("reality-opts")
        .and_then(|r| get_str(r, "public-key"))
        .map(str::to_string);
    let tls = if tls_flag || sni.is_some() || !alpn.is_empty() || reality.is_some() {
        Some(TlsSettings {
            enabled: tls_flag,
            sni,
            alpn,
            reality,
        })
    } else {
        None
    };

    Some(NormalizedNode {
        protocol,
        server,
        port,
        credentials,
        transport,
        tls,
        network_options: None,
        display_name: get_str(p, "name").map(str::to_string),
        source_context: ctx.clone(),
        raw_config_reference: serde_json::to_string(p).unwrap_or_default(),
        identity_hash: None,
        fingerprint_version: None,
    })
}

fn from_singbox(o: &Value, ctx: &SourceContext) -> Option<NormalizedNode> {
    let protocol = get_str(o, "type")?.to_lowercase();
    let server = get_str(o, "server")?.to_string();
    let port = get_u16(o, "server_port")?;
    let credentials = cred_for(&protocol, o)?;

    let transport = o.get("transport").and_then(|t| {
        let kind = get_str(t, "type")?;
        Some(Transport {
            kind: kind.to_string(),
            path: get_str(t, "path").map(str::to_string),
            host: t
                .get("headers")
                .and_then(|h| get_str(h, "Host").or_else(|| get_str(h, "host")))
                .map(str::to_string),
            service_name: get_str(t, "service_name").map(str::to_string),
        })
    });

    let tls = o.get("tls").map(|t| {
        let enabled = t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let sni = get_str(t, "server_name").map(str::to_string);
        let alpn = alpn_list(t);
        TlsSettings {
            enabled,
            sni,
            alpn,
            reality: None,
        }
    });

    Some(NormalizedNode {
        protocol,
        server,
        port,
        credentials,
        transport,
        tls,
        network_options: None,
        display_name: get_str(o, "tag").map(str::to_string),
        source_context: ctx.clone(),
        raw_config_reference: serde_json::to_string(o).unwrap_or_default(),
        identity_hash: None,
        fingerprint_version: None,
    })
}

fn parse_clash(text: &str, ctx: &SourceContext) -> Option<Vec<NormalizedNode>> {
    let v: Value = serde_yaml::from_str(text).ok()?;
    let proxies = v.get("proxies")?.as_array()?;
    let nodes: Vec<NormalizedNode> = proxies.iter().filter_map(|p| from_clash(p, ctx)).collect();
    Some(nodes)
}

fn parse_singbox(raw: &[u8], ctx: &SourceContext) -> Option<Vec<NormalizedNode>> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    let outbounds = v.get("outbounds")?.as_array()?;
    let nodes: Vec<NormalizedNode> = outbounds
        .iter()
        .filter_map(|o| from_singbox(o, ctx))
        .collect();
    Some(nodes)
}

fn try_base64(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let t = text.trim();
    let engines = [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
    ];
    for e in engines {
        if let Ok(d) = e.decode(t) {
            if !d.is_empty() {
                return Some(d);
            }
        }
    }
    None
}

/// Parse a subscription blob into normalized nodes.
pub fn parse_subscription(
    raw: &[u8],
    ctx: &SourceContext,
) -> Result<Vec<NormalizedNode>, AdapterError> {
    let text = String::from_utf8_lossy(raw).to_string();

    if let Some(nodes) = parse_clash(&text, ctx) {
        if !nodes.is_empty() {
            return Ok(nodes);
        }
    }
    if let Some(nodes) = parse_singbox(raw, ctx) {
        if !nodes.is_empty() {
            return Ok(nodes);
        }
    }
    if let Some(decoded) = try_base64(&text) {
        let decoded_text = String::from_utf8_lossy(&decoded).to_string();
        if let Some(nodes) = parse_clash(&decoded_text, ctx) {
            if !nodes.is_empty() {
                return Ok(nodes);
            }
        }
        if let Some(nodes) = parse_singbox(&decoded, ctx) {
            if !nodes.is_empty() {
                return Ok(nodes);
            }
        }
    }
    Err(AdapterError::UnrecognizedFormat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ctx() -> SourceContext {
        SourceContext {
            source_id: Uuid::nil(),
            fetch_run_id: Uuid::nil(),
        }
    }

    const CLASH: &str = r#"
proxies:
  - name: "hk-vmess-01"
    type: vmess
    server: hk.example.com
    port: 443
    uuid: 11111111-1111-1111-1111-111111111111
    alterId: 0
    cipher: auto
    tls: true
    network: ws
    ws-opts:
      path: "/ray"
      headers:
        Host: hk.example.com
  - name: "jp-vmess-02"
    type: vmess
    server: jp.example.com
    port: 443
    uuid: 22222222-2222-2222-2222-222222222222
    alterId: 0
    cipher: auto
  - name: "sg-trojan-01"
    type: trojan
    server: sg.example.com
    port: 443
    password: "s3cr3t-password"
    sni: sg.example.com
    alpn: ["h2"]
"#;

    const SINGBOX: &str = r#"{
      "outbounds": [
        {"type":"vmess","tag":"sb-vmess","server":"sb.example.com","server_port":8443,
         "uuid":"33333333-3333-3333-3333-333333333333",
         "transport":{"type":"ws","path":"/ws"},
         "tls":{"enabled":true,"server_name":"sb.example.com","alpn":["h2"]}},
        {"type":"trojan","tag":"sb-trojan","server":"t.example.com","server_port":443,
         "password":"tpw"}
      ]
    }"#;

    #[test]
    fn clash() {
        let nodes = parse_subscription(CLASH.as_bytes(), &ctx()).unwrap();
        assert_eq!(nodes.len(), 3);
        let names: Vec<_> = nodes
            .iter()
            .map(|n| n.display_name.clone().unwrap())
            .collect();
        assert_eq!(names, ["hk-vmess-01", "jp-vmess-02", "sg-trojan-01"]);
        assert_eq!(nodes[0].protocol, "vmess");
        assert_eq!(
            nodes[0].credentials,
            Credential::Uuid("11111111-1111-1111-1111-111111111111".into())
        );
        assert_eq!(
            nodes[0].transport.as_ref().unwrap().path.as_deref(),
            Some("/ray")
        );
        assert_eq!(
            nodes[0].transport.as_ref().unwrap().host.as_deref(),
            Some("hk.example.com")
        );
        assert_eq!(nodes[2].protocol, "trojan");
        assert_eq!(
            nodes[2].credentials,
            Credential::Password("s3cr3t-password".into())
        );
        assert_eq!(nodes[2].tls.as_ref().unwrap().alpn, vec!["h2".to_string()]);
    }

    #[test]
    fn singbox() {
        let nodes = parse_subscription(SINGBOX.as_bytes(), &ctx()).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].protocol, "vmess");
        assert_eq!(nodes[0].port, 8443);
        assert_eq!(nodes[0].transport.as_ref().unwrap().kind, "ws");
        assert_eq!(
            nodes[0].tls.as_ref().unwrap().sni.as_deref(),
            Some("sb.example.com")
        );
        assert_eq!(nodes[1].credentials, Credential::Password("tpw".into()));
    }

    #[test]
    fn base64() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(CLASH.as_bytes());
        let nodes = parse_subscription(encoded.as_bytes(), &ctx()).unwrap();
        assert_eq!(nodes.len(), 3, "base64(clash) must decode and parse");
    }

    #[test]
    fn field_separation() {
        let nodes = parse_subscription(CLASH.as_bytes(), &ctx()).unwrap();
        let mut renamed = nodes[0].clone();
        renamed.display_name = Some("totally-different".into());
        assert_eq!(
            nodes[0].identity_material(),
            renamed.identity_material(),
            "renaming must not change identity material"
        );
        let mut moved = nodes[0].clone();
        moved.transport.as_mut().unwrap().path = Some("/other".into());
        assert_ne!(
            nodes[0].identity_material(),
            moved.identity_material(),
            "transport change must change identity material"
        );
    }
}
