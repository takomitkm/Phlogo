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

/// Per-protocol primary credential extraction.
///
/// Mirrors the upstream Go `nodeAuth` switch (`internal/node/upsert.go`) **protocol for
/// protocol** — the credential that feeds the identity hash must be the same one
/// upstream picks, otherwise the same node hashes differently on either side.
/// Clash YAML and sing-box JSON use different field names for some protocols, so
/// each protocol lists its accepted fields per dialect.
fn cred_for(protocol: &str, obj: &Value) -> Option<Credential> {
    match protocol {
        // uuid
        "vmess" | "vless" => get_str(obj, "uuid").map(|s| Credential::Uuid(s.to_string())),
        // password (trojan/anytls/hysteria/hysteria2/ss/ssr all use `password`)
        "trojan" | "anytls" | "hysteria" | "hysteria2" => {
            get_str(obj, "password").map(|s| Credential::Password(s.to_string()))
        }
        // ss/ssr: upstream `nodeAuth` uses `password` ONLY — `cipher`/`method` is the
        // encryption algorithm (not a credential), so it must never feed the identity.
        "ss" | "shadowsocks" | "ssr" => {
            Some(Credential::Password(get_str(obj, "password").unwrap_or_default().to_string()))
        }
        // tuic: v5 carries uuid+password, v4 only password
        "tuic" => {
            let password = get_str(obj, "password").unwrap_or_default().to_string();
            match get_str(obj, "uuid") {
                Some(u) if !u.is_empty() => Some(Credential::Tuic {
                    uuid: u.to_string(),
                    password,
                }),
                _ if !password.is_empty() => Some(Credential::Tuic {
                    uuid: String::new(),
                    password,
                }),
                _ => None,
            }
        }
        // wireguard: private key
        "wireguard" => get_str(obj, "private-key")
            .or_else(|| get_str(obj, "private_key"))
            .map(|s| Credential::PrivateKey(s.to_string())),
        // snell: psk
        "snell" => get_str(obj, "psk").map(|s| Credential::Psk(s.to_string())),
        // socks5 / http / mieru: username + password
        "socks5" | "socks" | "http" | "https" | "mieru" => {
            let username = get_str(obj, "username").unwrap_or_default().to_string();
            let password = get_str(obj, "password").unwrap_or_default().to_string();
            if username.is_empty() && password.is_empty() {
                None
            } else {
                Some(Credential::UsernamePassword { username, password })
            }
        }
        // sudoku: key
        "sudoku" => get_str(obj, "key").map(|s| Credential::Key(s.to_string())),
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


    /// Protocol coverage regression (2026-11-13): the real school-machine
    /// subscription (mihomo v1.19.32) contains 36 vless + 16 hysteria2 — the
    /// hysteria2 nodes were silently dropped because `cred_for` only knew
    /// vmess/vless/trojan/ss. Now every protocol upstream `nodeAuth` supports
    /// must be extractable.
    const CLASH_MODERN: &str = r#"
proxies:
  - { name: "hy2-01", type: hysteria2, server: hy2.example.com, port: 443, password: "hy2-pw", sni: hy2.example.com, skip-cert-verify: true }
  - { name: "tuic-01", type: tuic, server: tuic.example.com, port: 443, uuid: "uu-11", password: "tuic-pw" }
  - { name: "anytls-01", type: anytls, server: at.example.com, port: 443, password: "at-pw" }
  - { name: "ss-01", type: ss, server: ss.example.com, port: 443, cipher: aes-256-gcm, password: "ss-pw" }
  - { name: "wg-01", type: wireguard, server: wg.example.com, port: 51820, private-key: "wg-key" }
  - { name: "snell-01", type: snell, server: snell.example.com, port: 443, psk: "snell-psk" }
  - { name: "socks-01", type: socks5, server: sk.example.com, port: 1080, username: "u1", password: "p1" }
"#;

    #[test]
    fn modern_protocols_are_parsed() {
        let nodes = parse_subscription(CLASH_MODERN.as_bytes(), &ctx()).unwrap();
        let by_name: std::collections::HashMap<_, _> = nodes
            .iter()
            .map(|n| (n.display_name.clone().unwrap_or_default(), n))
            .collect();
        assert_eq!(nodes.len(), 7, "all modern protocols must parse");

        assert_eq!(
            by_name["hy2-01"].credentials,
            Credential::Password("hy2-pw".into()),
            "hysteria2 uses password"
        );
        assert_eq!(
            by_name["tuic-01"].credentials,
            Credential::Tuic {
                uuid: "uu-11".into(),
                password: "tuic-pw".into()
            }
        );
        assert_eq!(
            by_name["anytls-01"].credentials,
            Credential::Password("at-pw".into())
        );
        assert_eq!(
            by_name["ss-01"].credentials,
            Credential::Password("ss-pw".into()),
            "ss identity uses password only — cipher/method is NOT a credential"
        );
        assert_eq!(
            by_name["wg-01"].credentials,
            Credential::PrivateKey("wg-key".into())
        );
        assert_eq!(by_name["snell-01"].credentials, Credential::Psk("snell-psk".into()));
        assert_eq!(
            by_name["socks-01"].credentials,
            Credential::UsernamePassword {
                username: "u1".into(),
                password: "p1".into()
            }
        );
    }

    /// The identity credential must match the upstream `nodeAuth` switch exactly,
    /// otherwise the same node hashes differently on either side.
    #[test]
    fn credentials_match_upstream_node_auth() {
        let nodes = parse_subscription(CLASH_MODERN.as_bytes(), &ctx()).unwrap();
        for n in &nodes {
            let auth = domain::fingerprint::node_auth(&n.protocol, &n.credentials);
            assert!(!auth.is_empty(), "{} produced an empty auth", n.protocol);
        }
        let hy2 = nodes
            .iter()
            .find(|n| n.protocol == "hysteria2")
            .expect("hysteria2 node");
        assert_eq!(domain::fingerprint::node_auth(&hy2.protocol, &hy2.credentials), "hy2-pw");
        let tuic = nodes.iter().find(|n| n.protocol == "tuic").expect("tuic node");
        assert_eq!(
            domain::fingerprint::node_auth(&tuic.protocol, &tuic.credentials),
            "uu-11:tuic-pw",
            "tuic v5 auth is uuid:password"
        );
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
