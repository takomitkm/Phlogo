//! URI share-link parsing (M1c closed loop): a subscription of one URI per line is a
//! real-world distribution format, and our own `distribution::uri` emits exactly that.
//!
//! Scope mirrors the URIs we generate (ss / vmess / vless / trojan / hysteria2 / tuic /
//! socks5 / http). Unknown schemes are skipped (reported in `parse_report`), never
//! silently turned into another protocol. Wire format notes:
//! - ss: `ss://base64url(method:password)@host:port#tag` (SIP002 userinfo)
//! - vmess: `vmess://base64(JSON)` — v2 fields add/port/id/aid/net/type/host/path/tls/sni
//! - the rest: `scheme://userinfo@host:port?query#tag`
//!
//! Parsing NEVER logs credentials; failures only name the scheme or line number.
use base64::Engine;
use domain::{Credential, NormalizedNode, SourceContext, TlsSettings, Transport};
use serde_json::json;

/// Percent-decoding that tolerates malformed escapes (passes them through verbatim).
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Split `userinfo@host:port/path?query#fragment` (no scheme).
struct UrlParts {
    userinfo: Option<String>,
    host: String,
    port: u16,
    query: Vec<(String, String)>,
    fragment: String,
}

fn split_url(rest: &str) -> Option<UrlParts> {
    let (rest, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, pct_decode(b)),
        None => (rest, String::new()),
    };
    let (rest, query_str) = match rest.split_once('?') {
        Some((a, b)) => (a, b),
        None => (rest, ""),
    };
    let mut query = Vec::new();
    for pair in query_str.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query.push((pct_decode(k), pct_decode(v)));
    }

    let (authority_part, _path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (userinfo, hostport) = match authority_part.rsplit_once('@') {
        Some((u, h)) => (Some(pct_decode(u)), h),
        None => (None, authority_part),
    };
    // IPv6 literal: everything between the last '[' ... ']' is the host.
    let (host, port_str) = if let Some(start) = hostport.find('[') {
        let end = hostport.find(']')?;
        let host = hostport[start + 1..end].to_string();
        let port = hostport[end + 1..].strip_prefix(':')?;
        (host, port.to_string())
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => (hostport.to_string(), String::new()),
        }
    };
    let port = if port_str.is_empty() {
        // No port in the URI: default per scheme is applied by the caller; 443 here.
        443
    } else {
        port_str.parse().ok()?
    };
    Some(UrlParts {
        userinfo,
        host,
        port,
        query,
        fragment,
    })
}

fn q_get(query: &[(String, String)], key: &str) -> Option<String> {
    query.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn transport_from_query(query: &[(String, String)]) -> Option<Transport> {
    let kind = q_get(query, "type").unwrap_or_else(|| "tcp".into());
    if kind == "tcp" || kind.is_empty() {
        return None;
    }
    Some(Transport {
        kind,
        path: q_get(query, "path"),
        host: q_get(query, "host"),
        service_name: q_get(query, "serviceName"),
    })
}

fn tls_from_query(query: &[(String, String)]) -> Option<TlsSettings> {
    let security = q_get(query, "security").unwrap_or_default();
    let sni = q_get(query, "sni").or_else(|| q_get(query, "peer"));
    let reality = q_get(query, "pbk");
    let tls_on = security == "tls" || security == "reality" || sni.is_some() || reality.is_some();
    if !tls_on {
        return None;
    }
    Some(TlsSettings {
        enabled: true,
        sni,
        alpn: q_get(query, "alpn")
            .map(|a| a.split(',').map(str::to_string).collect())
            .unwrap_or_default(),
        reality,
    })
}

fn display_name(fragment: &str) -> Option<String> {
    if fragment.is_empty() {
        None
    } else {
        Some(fragment.to_string())
    }
}

fn build_node(
    protocol: &str,
    parts: &UrlParts,
    credentials: Option<Credential>,
    ctx: &SourceContext,
) -> Option<NormalizedNode> {
    let transport = transport_from_query(&parts.query);
    let tls = tls_from_query(&parts.query);
    let raw = json!({
        "name": parts.fragment,
        "type": protocol,
        "server": parts.host,
        "port": parts.port,
    });
    let mut node = NormalizedNode {
        protocol: protocol.to_string(),
        server: parts.host.clone(),
        port: parts.port,
        credentials: credentials?,
        transport,
        tls,
        network_options: None,
        display_name: display_name(&parts.fragment),
        source_context: ctx.clone(),
        raw_config_reference: raw.to_string(),
        identity_hash: None,
        fingerprint_version: None,
    };
    node.rebuild_raw();
    Some(node)
}

/// Rebuild the persisted Clash-shaped payload from the normalized fields, so the
/// identity hash and the re-render path (base64) behave like every other node.
trait RebuildRaw {
    fn rebuild_raw(&mut self);
}

impl RebuildRaw for NormalizedNode {
    fn rebuild_raw(&mut self) {
        let Ok(mut obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            &self.raw_config_reference,
        ) else {
            return;
        };
        if let Some(t) = &self.transport {
            obj.insert("network".into(), json!(t.kind));
            if t.kind == "ws" {
                let mut ws = serde_json::Map::new();
                if let Some(p) = &t.path {
                    ws.insert("path".into(), json!(p));
                }
                if let Some(h) = &t.host {
                    ws.insert("headers".into(), json!({"Host": h}));
                }
                if !ws.is_empty() {
                    obj.insert("ws-opts".into(), serde_json::Value::Object(ws));
                }
            } else if t.kind == "grpc" {
                if let Some(s) = &t.service_name {
                    obj.insert("grpc-opts".into(), json!({"grpc-service-name": s}));
                }
            }
        }
        if let Some(tls) = &self.tls {
            obj.insert("tls".into(), json!(tls.enabled));
            if let Some(s) = &tls.sni {
                obj.insert("servername".into(), json!(s));
            }
            if !tls.alpn.is_empty() {
                obj.insert("alpn".into(), json!(tls.alpn));
            }
            if let Some(p) = &tls.reality {
                obj.insert(
                    "reality-opts".into(),
                    json!({"public-key": p, "short-id": ""}),
                );
            }
        }
        match &self.credentials {
            Credential::Uuid(u) => {
                obj.insert("uuid".into(), json!(u));
            }
            Credential::Password(p) => {
                obj.insert("password".into(), json!(p));
            }
            Credential::Tuic { uuid, password } => {
                obj.insert("uuid".into(), json!(uuid));
                obj.insert("password".into(), json!(password));
            }
            Credential::UsernamePassword { username, password } => {
                obj.insert("username".into(), json!(username));
                obj.insert("password".into(), json!(password));
            }
            _ => {}
        }
        self.raw_config_reference =
            serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_default();
    }
}

/// Parse one URI line. `None` = scheme unknown/unsupported or malformed (skipped).
pub fn parse_uri_line(line: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
        return None;
    }
    let (scheme, rest) = line.split_once("://")?;
    match scheme.to_ascii_lowercase().as_str() {
        "ss" => parse_ss(rest, ctx),
        "vmess" => parse_vmess(rest, ctx),
        "vless" => parse_vless(rest, ctx),
        "trojan" => parse_trojan(rest, ctx),
        "hysteria2" | "hy2" => parse_hysteria2(rest, ctx),
        "tuic" => parse_tuic(rest, ctx),
        "socks5" | "socks" => parse_socks5(rest, ctx),
        "http" => parse_http(rest, ctx),
        _ => None,
    }
}

fn parse_ss(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    // SIP002: base64url(method:password)@host:port
    let (userinfo, hostport) = rest.split_once('@')?;
    let method_pass = String::from_utf8(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(pct_decode(userinfo))
            .ok()?,
    )
    .ok()?;
    let (cipher, password) = method_pass.split_once(':')?;
    let mut parts = split_url(hostport)?;
    parts.userinfo = None;
    if parts.port == 0 {
        parts.port = 443;
    }
    let node = build_node(
        "ss",
        &parts,
        Some(Credential::Password(password.to_string())),
        ctx,
    )?;
    // cipher is an encryption algorithm, not a credential: keep it in the payload.
    let mut obj = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
        &node.raw_config_reference,
    )
    .unwrap_or_default();
    obj.insert("cipher".into(), json!(cipher));
    let node = NormalizedNode {
        raw_config_reference: serde_json::Value::Object(obj).to_string(),
        ..node
    };
    // ss-over-URI carries transport only via plugin (unsupported here): strip any
    // accidental query-derived transport/TLS so the payload stays honest.
    Some(NormalizedNode {
        transport: None,
        tls: None,
        ..node
    })
}

fn parse_vmess(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(rest.trim())
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(rest.trim())
                .ok()
        })?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let uuid = v.get("id").and_then(|x| x.as_str())?;
    let server = v.get("add").and_then(|x| x.as_str())?.to_string();
    let port = v
        .get("port")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())?;
    let net = v.get("net").and_then(|x| x.as_str()).unwrap_or("tcp");
    let tls_on = v.get("tls").and_then(|x| x.as_str()) == Some("tls");
    let sni = v
        .get("sni")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty());
    let ws_path = if net == "ws" {
        v.get("path")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let ws_host = v
        .get("host")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty());
    let transport = (net != "tcp").then(|| Transport {
        kind: net.to_string(),
        path: ws_path.map(str::to_string),
        host: ws_host.map(str::to_string),
        service_name: None,
    });
    let tls = if tls_on || sni.is_some() {
        Some(TlsSettings {
            enabled: tls_on,
            sni: sni.map(str::to_string),
            alpn: vec![],
            reality: None,
        })
    } else {
        None
    };
    let raw = json!({
        "name": v.get("ps").and_then(|x| x.as_str()).unwrap_or(""),
        "type": "vmess", "server": server, "port": port,
        "uuid": uuid.to_string(),
        "alterId": v.get("aid").and_then(|x| x.as_str()).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0),
        "cipher": v.get("scy").and_then(|x| x.as_str()).unwrap_or("auto"),
        "network": net, "tls": tls_on,
    });
    let uuid_owned = uuid.to_string();
    let mut node = NormalizedNode {
        protocol: "vmess".into(),
        server: server.clone(),
        port,
        credentials: Credential::Uuid(uuid_owned),
        transport,
        tls,
        network_options: None,
        display_name: v.get("ps").and_then(|x| x.as_str()).map(str::to_string),
        source_context: ctx.clone(),
        raw_config_reference: raw.to_string(),
        identity_hash: None,
        fingerprint_version: None,
    };
    if let Some(t) = &node.transport {
        // Reuse the clash-shaped reconstruction for ws-opts.
        if let Ok(mut o) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            &node.raw_config_reference,
        ) {
            if let Some(p) = &t.path {
                o.insert(
                    "ws-opts".into(),
                    json!({"path": p, "headers": {"Host": t.host.clone().unwrap_or_default()}}),
                );
            }
            node.raw_config_reference = serde_json::Value::Object(o).to_string();
        }
    }
    Some(node)
}

fn parse_vless(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let (userinfo, rest) = rest.split_once('@')?;
    let uuid = pct_decode(userinfo);
    let mut parts = split_url(rest)?;
    parts.userinfo = None;
    if uuid.is_empty() {
        return None;
    }
    let mut node = build_node("vless", &parts, Some(Credential::Uuid(uuid)), ctx)?;
    // flow is protocol-specific; keep it in the payload for identity stability.
    if let Ok(mut obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
        &node.raw_config_reference,
    ) {
        if let Some(f) = q_get(&parts.query, "flow") {
            obj.insert("flow".into(), json!(f));
        }
        node.raw_config_reference = serde_json::Value::Object(obj).to_string();
    }
    Some(node)
}

fn parse_trojan(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let (userinfo, rest) = rest.split_once('@')?;
    let password = pct_decode(userinfo);
    let mut parts = split_url(rest)?;
    parts.userinfo = None;
    if password.is_empty() {
        return None;
    }
    build_node("trojan", &parts, Some(Credential::Password(password)), ctx)
}

fn parse_hysteria2(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let (userinfo, rest) = rest.split_once('@')?;
    let password = pct_decode(userinfo);
    let mut parts = split_url(rest)?;
    parts.userinfo = None;
    if password.is_empty() {
        return None;
    }
    let mut node = build_node(
        "hysteria2",
        &parts,
        Some(Credential::Password(password)),
        ctx,
    )?;
    if let Ok(mut obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
        &node.raw_config_reference,
    ) {
        if let Some(o) = q_get(&parts.query, "obfs") {
            obj.insert("obfs".into(), json!(o));
        }
        if let Some(p) = q_get(&parts.query, "obfs-password") {
            obj.insert("obfs-password".into(), json!(p));
        }
        if q_get(&parts.query, "insecure").as_deref() == Some("1") {
            obj.insert("skip-cert-verify".into(), json!(true));
        }
        node.raw_config_reference = serde_json::Value::Object(obj).to_string();
    }
    Some(node)
}

fn parse_tuic(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let (userinfo, rest) = rest.split_once('@')?;
    let (uuid, password) = userinfo.split_once(':')?;
    let mut parts = split_url(rest)?;
    parts.userinfo = None;
    let credentials = Some(Credential::Tuic {
        uuid: pct_decode(uuid),
        password: pct_decode(password),
    });
    let mut node = build_node("tuic", &parts, credentials, ctx)?;
    if let Ok(mut obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
        &node.raw_config_reference,
    ) {
        if let Some(c) = q_get(&parts.query, "congestion_control") {
            obj.insert("congestion-controller".into(), json!(c));
        }
        if let Some(m) = q_get(&parts.query, "udp_relay_mode") {
            obj.insert("udp-relay-mode".into(), json!(m));
        }
        node.raw_config_reference = serde_json::Value::Object(obj).to_string();
    }
    Some(node)
}

fn parse_socks5(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    parse_user_pass("socks5", rest, ctx)
}

fn parse_http(rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    parse_user_pass("http", rest, ctx)
}

fn parse_user_pass(protocol: &str, rest: &str, ctx: &SourceContext) -> Option<NormalizedNode> {
    let (userinfo, hostport) = match rest.split_once('@') {
        Some((u, h)) => (pct_decode(u), h.to_string()),
        None => (String::new(), rest.to_string()),
    };
    let (username, password) = userinfo.split_once(':').unwrap_or(("", ""));
    let mut parts = split_url(&hostport)?;
    parts.userinfo = None;
    let cred = if username.is_empty() && password.is_empty() {
        // No userinfo = no usable credential: skip instead of parsing a ghost node.
        return None;
    } else {
        Some(Credential::UsernamePassword {
            username: username.to_string(),
            password: password.to_string(),
        })
    };
    build_node(protocol, &parts, cred, ctx)
}

/// Parse a URI-list subscription body (one link per line, optional base64 wrapper is
/// handled by [`parse_subscription`]).
pub fn parse_uri_list(text: &str, ctx: &SourceContext) -> Vec<NormalizedNode> {
    text.lines()
        .filter_map(|line| parse_uri_line(line, ctx))
        .collect()
}

/// Looks like a URI-list body: every non-empty, non-comment line starts with a
/// `scheme://` we might support.
pub fn looks_like_uri_list(text: &str) -> bool {
    let mut saw_any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        saw_any = true;
        let Some((scheme, rest)) = line.split_once("://") else {
            return false;
        };
        if rest.is_empty()
            || scheme.len() > 16
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+')
        {
            return false;
        }
    }
    saw_any
}

#[cfg(test)]
mod uri_tests {
    use super::*;
    use uuid::Uuid;

    fn ctx() -> SourceContext {
        SourceContext {
            source_id: Uuid::nil(),
            fetch_run_id: Uuid::nil(),
        }
    }

    #[test]
    fn trojan_roundtrip_shape() {
        let n = parse_uri_line(
            "trojan://p%40ss%20w@e.example:443?security=tls&sni=e.example&type=ws&path=%2Fws&host=cdn.example#JP%2001",
            &ctx(),
        )
        .expect("trojan");
        assert_eq!(n.protocol, "trojan");
        assert_eq!(n.server, "e.example");
        assert_eq!(n.port, 443);
        match &n.credentials {
            Credential::Password(p) => assert_eq!(p, "p@ss w"),
            other => panic!("cred: {other:?}"),
        }
        let t = n.transport.expect("ws transport");
        assert_eq!(t.kind, "ws");
        assert_eq!(t.path.as_deref(), Some("/ws"));
        assert_eq!(t.host.as_deref(), Some("cdn.example"));
        let tls = n.tls.expect("tls");
        assert!(tls.enabled);
        assert_eq!(tls.sni.as_deref(), Some("e.example"));
        assert_eq!(n.display_name.as_deref(), Some("JP 01"));
    }

    #[test]
    fn ss_sip002_roundtrip() {
        // base64url (no padding) of "aes-256-gcm:p@ss"
        let raw = "ss://YWVzLTI1Ni1nY206cEBzcw@a.example:8388#SS01";
        let n = parse_uri_line(raw, &ctx()).expect("ss canonical");
        assert_eq!(n.protocol, "ss");
        match &n.credentials {
            Credential::Password(p) => assert_eq!(p, "p@ss"),
            other => panic!("cred: {other:?}"),
        }
        assert_eq!(n.server, "a.example");
        assert_eq!(n.port, 8388);
        let payload: serde_json::Value = serde_json::from_str(&n.raw_config_reference).unwrap();
        assert_eq!(payload["cipher"], "aes-256-gcm");
    }

    #[test]
    fn vmess_roundtrip() {
        let body = serde_json::json!({
            "v": "2", "ps": "VM01", "add": "b.example", "port": "443",
            "id": "11111111-1111-1111-1111-111111111111", "aid": "0", "scy": "auto",
            "net": "ws", "type": "none", "host": "cdn.example", "path": "/ray",
            "tls": "tls", "sni": "b.example"
        })
        .to_string();
        use base64::Engine;
        let uri = format!(
            "vmess://{}",
            base64::engine::general_purpose::STANDARD.encode(body)
        );
        let n = parse_uri_line(&uri, &ctx()).expect("vmess");
        assert_eq!(n.protocol, "vmess");
        assert_eq!(n.server, "b.example");
        assert_eq!(n.port, 443);
        match &n.credentials {
            Credential::Uuid(u) => assert_eq!(u, "11111111-1111-1111-1111-111111111111"),
            other => panic!("cred: {other:?}"),
        }
        let t = n.transport.expect("ws");
        assert_eq!(t.path.as_deref(), Some("/ray"));
        let tls = n.tls.expect("tls");
        assert!(tls.enabled);
        let payload: serde_json::Value = serde_json::from_str(&n.raw_config_reference).unwrap();
        assert_eq!(payload["alterId"], 0);
        assert_eq!(payload["cipher"], "auto");
        assert_eq!(payload["ws-opts"]["path"], "/ray");
    }

    #[test]
    fn vless_reality_fields_survive() {
        let n = parse_uri_line(
            "vless://22222222-2222-2222-2222-222222222222@c.example:443?security=reality&sni=c.example&fp=chrome&pbk=PBKVALUE&sid=ab&flow=xtls-rprx-vision&type=tcp#R01",
            &ctx(),
        )
        .expect("vless");
        let tls = n.tls.expect("reality");
        assert_eq!(tls.reality.as_deref(), Some("PBKVALUE"));
        let payload: serde_json::Value = serde_json::from_str(&n.raw_config_reference).unwrap();
        assert_eq!(payload["flow"], "xtls-rprx-vision");
        assert_eq!(payload["reality-opts"]["public-key"], "PBKVALUE");
    }

    #[test]
    fn hysteria2_and_tuic_keep_extras() {
        let hy2 = parse_uri_line(
            "hysteria2://pw@f.example:8443?sni=f.example&obfs=salamander&obfs-password=op&insecure=1#H01",
            &ctx(),
        )
        .expect("hy2");
        let payload: serde_json::Value = serde_json::from_str(&hy2.raw_config_reference).unwrap();
        assert_eq!(payload["obfs"], "salamander");
        assert_eq!(payload["obfs-password"], "op");
        assert_eq!(payload["skip-cert-verify"], true);

        let tuic = parse_uri_line(
            "tuic://44444444-4444-4444-4444-444444444444:tp@g.example:443?congestion_control=bbr&udp_relay_mode=native&alpn=h3#U01",
            &ctx(),
        )
        .expect("tuic");
        match &tuic.credentials {
            Credential::Tuic { uuid, password } => {
                assert_eq!(uuid, "44444444-4444-4444-4444-444444444444");
                assert_eq!(password, "tp");
            }
            other => panic!("cred: {other:?}"),
        }
        let payload: serde_json::Value = serde_json::from_str(&tuic.raw_config_reference).unwrap();
        assert_eq!(payload["congestion-controller"], "bbr");
        assert_eq!(payload["udp-relay-mode"], "native");
    }

    #[test]
    fn socks5_and_http_userinfo() {
        let n = parse_uri_line("socks5://u%40x:p%23w@h.example:1080#S", &ctx()).expect("socks");
        match &n.credentials {
            Credential::UsernamePassword { username, password } => {
                assert_eq!(username, "u@x");
                assert_eq!(password, "p#w");
            }
            other => panic!("cred: {other:?}"),
        }
        // No userinfo at all: there is no credential, so the node is skipped (returns
        // None) rather than parsed into a credential-less ghost.
        assert!(
            parse_uri_line("http://h.example:8080#PLAIN", &ctx()).is_none(),
            "credential-less http must be skipped, not parsed"
        );
    }

    #[test]
    fn ipv6_literal_is_parsed() {
        let n =
            parse_uri_line("trojan://p@[2001:db8::1]:443?sni=v6.example#V6", &ctx()).expect("v6");
        assert_eq!(n.server, "2001:db8::1");
        assert_eq!(n.port, 443);
    }

    #[test]
    fn unknown_and_malformed_are_none() {
        assert!(
            parse_uri_line("ssr://whatever", &ctx()).is_none(),
            "ssr unsupported"
        );
        assert!(parse_uri_line("mieru://x", &ctx()).is_none());
        assert!(parse_uri_line("trojan://no-authority", &ctx()).is_none());
        assert!(parse_uri_line("not-a-uri", &ctx()).is_none());
        assert!(parse_uri_line("", &ctx()).is_none());
        assert!(parse_uri_line("# comment", &ctx()).is_none());
    }

    #[test]
    fn uri_list_and_detection() {
        let body =
            "trojan://pw@a.example:443#A\n# note\n\nss://YWVzLTI1Ni1nY206cEBzcw@b.example:8388#B\n";
        assert!(looks_like_uri_list(body));
        let nodes = parse_uri_list(body, &ctx());
        assert_eq!(nodes.len(), 2, "comment and blank skipped");
        assert_eq!(nodes[0].protocol, "trojan");
        assert_eq!(nodes[1].protocol, "ss");
        assert!(!looks_like_uri_list("proxies:\n  - name: x"));
    }

    /// Round-trip the URIs OUR OWN builder emits back into normalized nodes and compare
    /// field by field (credentials included, never logged).
    #[test]
    fn builder_output_roundtrips_through_the_parser() {
        use distribution::uri::uri_from_clash;
        let cases: Vec<(serde_json::Value, &str, &str, u16)> = vec![
            (
                serde_json::json!({"type":"trojan","server":"e.example","port":443,"password":"p@ss w","servername":"e.example","name":"T01"}),
                "trojan",
                "e.example",
                443,
            ),
            (
                serde_json::json!({"type":"ss","server":"a.example","port":8388,"cipher":"aes-256-gcm","password":"p@ss","name":"S01"}),
                "ss",
                "a.example",
                8388,
            ),
            (
                serde_json::json!({"type":"hysteria2","server":"f.example","port":8443,"password":"pw","servername":"f.example","obfs":"salamander","obfs-password":"op","skip-cert-verify":true,"name":"H01"}),
                "hysteria2",
                "f.example",
                8443,
            ),
            (
                serde_json::json!({"type":"tuic","server":"g.example","port":443,"uuid":"44444444-4444-4444-4444-444444444444","password":"tp","servername":"g.example","congestion-controller":"bbr","name":"U01"}),
                "tuic",
                "g.example",
                443,
            ),
        ];
        for (payload, protocol, server, port) in cases {
            let uri = uri_from_clash(&payload).unwrap_or_else(|| panic!("{protocol} uri"));
            let n =
                parse_uri_line(&uri, &ctx()).unwrap_or_else(|| panic!("{protocol} parse: {uri}"));
            assert_eq!(n.protocol, protocol, "protocol for {uri}");
            assert_eq!(n.server, server, "server for {uri}");
            assert_eq!(n.port, port, "port for {uri}");
            let expected_cred = payload.get("password").and_then(|v| v.as_str());
            if let Some(want) = expected_cred {
                let got = match &n.credentials {
                    Credential::Password(p) => p.as_str(),
                    Credential::Tuic { password, .. } => password.as_str(),
                    other => panic!("cred shape for {protocol}: {other:?}"),
                };
                assert_eq!(got, want, "credential roundtrip for {protocol}");
            }
        }
    }
}
