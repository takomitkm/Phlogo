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

    // Nodes whose protocol sing-box cannot represent are **excluded** from the
    // sing-box output entirely (including from the selector/urltest lists) —
    // a selector referencing a missing tag makes sing-box refuse to start.
    let usable: Vec<&OutputNode> = nodes
        .iter()
        .filter(|n| n.singbox_outbound.get("type").and_then(Value::as_str) != Some("direct"))
        .collect();

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
                m.insert("outbounds".into(), json!(usable.iter().map(|n| n.name.clone()).collect::<Vec<_>>()));
            }
            kept.push(ob);
        } else {
            kept.push(ob.clone());
        }
    }
    for n in &usable {
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

/// Protocols that have **no sing-box outbound counterpart**.
///
/// - `mieru` / `sudoku` / `ssr` exist in mihomo but not in sing-box;
/// - `shadowtls` is a TLS *wrapper* (it cannot be a standalone outbound target —
///   it must wrap another outbound);
/// - `wireguard`: sing-box **removed the wireguard outbound in 1.13** (it moved to
///   the endpoint form, `option.WireGuardEndpointOptions`, which is not an
///   outbound). Verified against the sing-box v1.14.4 binary: `type: wireguard`
///   fails with "outbounds[0]: unknown outbound type".
///
/// Emitting them as sing-box outbounds would produce a config that sing-box
/// refuses to load, so they are **skipped** for the sing-box target (they remain
/// valid in the Clash/mihomo output).
const SINGBOX_UNSUPPORTED: &[&str] = &["mieru", "sudoku", "ssr", "shadowtls", "wireguard"];

/// True when this protocol can be represented as a sing-box outbound.
pub fn singbox_supports(protocol: &str) -> bool {
    !SINGBOX_UNSUPPORTED.contains(&protocol)
}

/// Parse a bandwidth value into an integer Mbps.
///
/// mihomo accepts `"100"`, `"100 Mbps"`, `"1g"`; sing-box wants a plain integer
/// (`up_mbps`/`down_mbps`).
fn mbps_of(v: &Value) -> Value {
    if let Some(n) = v.as_u64() {
        return json!(n);
    }
    let s = v.as_str().unwrap_or("").trim().to_lowercase();
    let num: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let n: f64 = num.parse().unwrap_or(0.0);
    let mult = if s.contains('g') {
        1000.0
    } else if s.contains('k') {
        0.001
    } else {
        1.0
    };
    json!((n * mult) as u64)
}

/// Render a duration-ish value as a sing-box duration string.
///
/// Units differ per field in mihomo, so the caller says which one it is:
/// - `hop-interval` is **seconds** (mihomo: `hop-interval: 30`);
/// - `heartbeat-interval` and the `idle-session-*` knobs are **milliseconds**
///   (mihomo: `heartbeat-interval: 10000`).
///
/// sing-box always wants a Go duration string (`"30s"`, `"10000ms"`).
fn duration_secs(v: &Value) -> Value {
    if let Some(s) = v.as_str() {
        return json!(s);
    }
    if let Some(n) = v.as_u64() {
        return json!(format!("{n}s"));
    }
    Value::Null
}

/// Like [`duration_secs`] but the source value is milliseconds.
fn duration_ms(v: &Value) -> Value {
    if let Some(s) = v.as_str() {
        return json!(s);
    }
    if let Some(n) = v.as_u64() {
        return json!(format!("{n}ms"));
    }
    Value::Null
}

/// Convert a mihomo port-range list into sing-box's `server_ports` array.
///
/// Normalise one port-range element to the form sing-box accepts.
///
/// sing-box's `parsePorts` (sing-quic `hysteria2/client.go`) **requires a colon in
/// every element**:
///
/// ```go
/// if !strings.Contains(portRange, ":") {
///     return nil, E.New("bad port range: ", portRange)
/// }
/// ```
///
/// so `"443"` (a bare single port, legal in mihomo) must become `"443:443"`.
/// A trailing colon is allowed and means "to 65535" (`"60000:"`).
fn normalize_port_range(p: &str) -> Option<String> {
    let s = p.trim().replace('-', ":");
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        Some(s)
    } else {
        Some(format!("{s}:{s}"))
    }
}

/// Convert a mihomo port-range list into sing-box's `server_ports` array.
///
/// - mihomo: `ports: "60000-65530"` (string, dash-separated, comma for several);
/// - sing-box: `server_ports: ["60000:65530"]` (array, **colon**-separated).
///
/// The colon is not a style choice: sing-box v1.14.4 rejects the dash form with
/// `bad port range: 60000-65530`, and rejects a bare port too. Verified with the
/// real binary (2026-11-13) — see docs/rebuild/13-*.md §5.6.
fn server_ports_of(v: &Value) -> Value {
    let raw: Vec<String> = match v {
        Value::Array(items) => items
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| x.to_string())
            })
            .collect(),
        Value::String(s) => s.split(',').map(|p| p.trim().to_string()).collect(),
        other => vec![other.to_string()],
    };
    let parts: Vec<String> = raw
        .iter()
        .filter_map(|p| normalize_port_range(p))
        .collect();
    json!(parts)
}

/// Protocols whose sing-box outbound has **no** `tls` block.
const SINGBOX_NO_TLS: &[&str] = &["shadowsocks", "ss", "snell", "socks", "socks5", "http", "https"];

/// Protocols whose sing-box outbound is a **deprecated** shape without
/// `server`/`server_port` (sing-box 1.11 deprecated the wireguard outbound; it
/// moved to the endpoint form and is removed in 1.13).
const SINGBOX_NO_ENDPOINT: &[&str] = &["wireguard"];

/// Build a sing-box outbound from a **Clash-shaped payload** (full fidelity).
///
/// The Clash payload is the source of truth for protocol-specific fields
/// (hysteria2 `password`/`obfs`/`ports`, tuic `uuid`+`password`, wireguard
/// `private-key`, shadowsocks `cipher`+`plugin`, snell `psk`+`version`, …). The
/// sing-box schema differs from Clash in field names, so this maps them per
/// protocol instead of hard-coding a vmess/vless/trojan-only shape.
///
/// Field names were taken from the **real** sources (mihomo `adapter/outbound/*.go`
/// struct tags, sing-box v1.14.0 outbound docs) and the output is validated by the
/// real sing-box binary — not by eyeballing a field table.
pub fn singbox_outbound_from_clash(payload: &Value, name: &str) -> Option<Value> {
    let mut ob = match payload {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    let protocol = ob
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if !singbox_supports(&protocol) {
        return None;
    }
    ob.remove("name");
    ob.insert("tag".into(), json!(name));

    // Clash `server`/`port` -> sing-box `server`/`server_port`.
    // (wireguard is excluded: its sing-box outbound has no endpoint fields.)
    if !SINGBOX_NO_ENDPOINT.contains(&protocol.as_str()) {
        if let Some(s) = ob.remove("server") {
            ob.insert("server".into(), s);
        }
        if let Some(p) = ob.remove("port") {
            ob.insert("server_port".into(), p);
        }
    } else {
        ob.remove("server");
        ob.remove("port");
    }

    // ---- protocol-specific parameter mapping ----
    match protocol.as_str() {
        "hysteria2" => {
            // mihomo: ports / mport / hop-interval / up / down / obfs + obfs-password
            // sing-box: server_ports / hop_interval / up_mbps / down_mbps / obfs{type,password}
            if let Some(v) = ob.remove("ports").or_else(|| ob.remove("mport")) {
                ob.insert("server_ports".into(), server_ports_of(&v));
            }
            if let Some(v) = ob.remove("hop-interval") {
                ob.insert("hop_interval".into(), duration_secs(&v));
            }
            // sing-box wants an integer Mbps; mihomo allows "100 Mbps" / "100".
            if let Some(v) = ob.remove("up") {
                ob.insert("up_mbps".into(), mbps_of(&v));
            }
            if let Some(v) = ob.remove("down") {
                ob.insert("down_mbps".into(), mbps_of(&v));
            }
            if let Some(v) = ob.remove("obfs") {
                // mihomo's `obfs` is the obfuscation TYPE (e.g. "salamander") and
                // `obfs-password` carries the password; sing-box splits them into
                // obfs.type + obfs.password.
                let pw = ob.remove("obfs-password").unwrap_or(Value::Null);
                ob.insert(
                    "obfs".into(),
                    json!({"type": v, "password": pw}),
                );
            } else {
                ob.remove("obfs-password");
            }
        }
        "hysteria" => {
            // mihomo hysteria v1: auth-str / obfs / up / down
            if let Some(v) = ob.remove("auth-str") {
                ob.insert("auth_str".into(), v);
            }
            if let Some(v) = ob.remove("up") {
                ob.insert("up_mbps".into(), mbps_of(&v));
            }
            if let Some(v) = ob.remove("down") {
                ob.insert("down_mbps".into(), mbps_of(&v));
            }
            if let Some(v) = ob.remove("ports") {
                ob.insert("server_ports".into(), server_ports_of(&v));
            }
            if let Some(v) = ob.remove("hop-interval") {
                ob.insert("hop_interval".into(), duration_secs(&v));
            }
        }
        "tuic" => {            if let Some(v) = ob.remove("congestion-controller") {
                ob.insert("congestion_control".into(), v);
            }
            if let Some(v) = ob.remove("udp-relay-mode") {
                ob.insert("udp_relay_mode".into(), v);
            }
            // mihomo heartbeat-interval is an integer (ms); sing-box wants a duration string.
            if let Some(v) = ob.remove("heartbeat-interval") {
                ob.insert("heartbeat".into(), duration_ms(&v));
            }
        }
        "shadowsocks" | "ss" => {
            // sing-box names the type `shadowsocks` and the cipher `method`.
            ob.insert("type".into(), json!("shadowsocks"));
            if let Some(c) = ob.remove("cipher") {
                ob.entry("method").or_insert(c);
            }
            // mihomo's plugin-opts is a map; sing-box wants a single string.
            if let Some(po) = ob.remove("plugin-opts") {
                let s = match &po {
                    Value::String(s) => s.clone(),
                    Value::Object(m) => m
                        .iter()
                        .map(|(k, v)| {
                            let val = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
                            format!("{k}={val}")
                        })
                        .collect::<Vec<_>>()
                        .join(";"),
                    other => other.to_string(),
                };
                ob.insert("plugin_opts".into(), json!(s));
            }
        }
        "snell" => {
            // mihomo: psk / version / obfs-opts{mode,host}
            // sing-box: psk / version / obfs_mode + obfs_host (v4) or mode (v6)
            if let Some(oo) = ob.remove("obfs-opts") {
                if let Some(m) = oo.get("mode").cloned() {
                    ob.insert("obfs_mode".into(), m);
                }
                if let Some(h) = oo.get("host").cloned() {
                    ob.insert("obfs_host".into(), h);
                }
            }
        }
        "socks5" | "socks" => {
            // sing-box calls it `socks` and needs an explicit version.
            ob.insert("type".into(), json!("socks"));
            ob.entry("version").or_insert(json!("5"));
        }
        "wireguard" => {
            // sing-box spells wireguard's key `private_key`; Clash uses `private-key`.
            if let Some(k) = ob.remove("private-key") {
                ob.entry("private_key").or_insert(k);
            }
            if let Some(v) = ob.remove("pre-shared-key") {
                ob.entry("pre_shared_key").or_insert(v);
            }
            if let Some(v) = ob.remove("public-key") {
                ob.entry("peer_public_key").or_insert(v);
            }
        }
        _ => {}
    }

    // Credentials: keep the native field names (they match between dialects for
    // every protocol we support) — the parser guarantees the primary one exists.

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
        "tls", "fingerprint", "mport", "smux", "plugin", "underlying-proxy", "ip-version",
        // mihomo-only knobs with no sing-box counterpart
        "protocol", "ech-opts", "ca", "ca-str", "cwnd", "udp-mtu", "reduce-rtt",
        "request-timeout", "disable-sni", "max-udp-relay-packet-size", "fast-open",
        "max-open-streams", "recv-window-conn", "recv-window", "disable-mtu-discovery",
        "max-datagram-frame-size", "udp-over-stream", "udp-over-stream-version",
        "initial-stream-receive-window", "max-stream-receive-window",
        "initial-connection-receive-window", "max-connection-receive-window",
        "idle-session-check-interval", "idle-session-timeout", "min-idle-session",
        "client-metadata", "persistent-keepalive", "remote-dns-resolve",
        "refresh-server-ip-interval", "amnezia-wg-option", "jc", "jmin", "jmax",
        "s1", "s2", "h1", "h2", "h3", "h4", "ip", "ipv6",
        // wireguard: sing-box's deprecated outbound has no `reserved`/`peers` shape
        "reserved", "peers", "public-key", "pre-shared-key", "allowed-ips",
        "mtu", "workers", "system-interface", "interface-name", "local-address",
        "gso", "peer-public-key", "pre-shared-key",
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

    // Protocols without a TLS concept in sing-box must not carry a `tls` object
    // (sing-box rejects the unknown field).
    if SINGBOX_NO_TLS.contains(&protocol.as_str()) {
        ob.remove("tls");
    }

    Some(Value::Object(ob))
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
        let sb = singbox_outbound_from_clash(&payload, "hy2-01").expect("supported protocol");
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
        let sb = singbox_outbound_from_clash(&tuic, "t").expect("supported protocol");
        assert_eq!(sb["uuid"], "uu");
        assert_eq!(sb["password"], "pw");

        let ss = serde_json::json!({
            "name": "s", "type": "ss", "server": "s.example.com", "port": 443,
            "cipher": "aes-256-gcm", "password": "sp"
        });
        let sb = singbox_outbound_from_clash(&ss, "s").expect("supported protocol");
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
        let sb = singbox_outbound_from_clash(&payload, "v").expect("supported protocol");
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
        let sb = singbox_outbound_from_clash(&payload, "r").expect("supported protocol");
        assert_eq!(sb["tls"]["utls"]["enabled"], true);
        assert_eq!(sb["tls"]["utls"]["fingerprint"], "ios");
        assert_eq!(sb["flow"], "xtls-rprx-vision", "vision flow must survive");
        assert!(sb.get("client-fingerprint").is_none(), "clash key must be consumed");
    }

    /// Protocol gaps closed in S54 — every field below was found by validating
    /// generated configs with the REAL sing-box v1.14.4 and mihomo v1.19.32
    /// binaries on the school machine (not by reading field tables).
    #[test]
    fn singbox_maps_hysteria2_port_hopping_and_obfs() {
        let payload = serde_json::json!({
            "name": "hy", "type": "hysteria2", "server": "h.example.com", "port": 443,
            "password": "pw", "sni": "h.example.com",
            "ports": "60000-65530", "hop-interval": 30,
            "up": "100 Mbps", "down": "200 Mbps",
            "obfs": "salamander", "obfs-password": "obfs-pw"
        });
        let sb = singbox_outbound_from_clash(&payload, "hy").expect("hysteria2");
        assert_eq!(
            sb["server_ports"],
            serde_json::json!(["60000:65530"]),
            "mihomo '60000-65530' -> sing-box array with colon ranges"
        );
        assert_eq!(sb["hop_interval"], "30s", "mihomo seconds -> sing-box duration string");
        assert_eq!(sb["up_mbps"], 100, "'100 Mbps' -> integer Mbps");
        assert_eq!(sb["down_mbps"], 200);
        assert_eq!(sb["obfs"]["type"], "salamander");
        assert_eq!(sb["obfs"]["password"], "obfs-pw");
    }

    #[test]
    fn singbox_maps_tuic_congestion_and_heartbeat() {
        let payload = serde_json::json!({
            "name": "t", "type": "tuic", "server": "t.example.com", "port": 443,
            "uuid": "bf000d23-0752-40b4-affe-68f7707a9661", "password": "pw",
            "congestion-controller": "cubic", "udp-relay-mode": "native",
            "heartbeat-interval": 10000
        });
        let sb = singbox_outbound_from_clash(&payload, "t").expect("tuic");
        assert_eq!(sb["congestion_control"], "cubic");
        assert_eq!(sb["udp_relay_mode"], "native");
        assert_eq!(sb["heartbeat"], "10000ms", "mihomo int ms -> sing-box duration");
    }

    #[test]
    fn singbox_maps_shadowsocks_type_and_plugin_opts() {
        let payload = serde_json::json!({
            "name": "s", "type": "ss", "server": "s.example.com", "port": 8388,
            "cipher": "aes-256-gcm", "password": "pw",
            "plugin": "obfs", "plugin-opts": {"mode": "http", "host": "x.com"}
        });
        let sb = singbox_outbound_from_clash(&payload, "s").expect("ss");
        assert_eq!(sb["type"], "shadowsocks", "sing-box has no 'ss' outbound type");
        assert_eq!(sb["method"], "aes-256-gcm");
        assert!(
            sb["plugin_opts"].is_string(),
            "sing-box plugin_opts is a string, mihomo's is a map"
        );
        assert!(sb["plugin_opts"].as_str().unwrap().contains("mode=http"));
    }

    #[test]
    fn singbox_drops_tls_for_tls_less_protocols() {
        for proto in ["shadowsocks", "snell", "socks5"] {
            let payload = serde_json::json!({
                "name": "x", "type": proto, "server": "x.example.com", "port": 1080,
                "password": "pw", "sni": "x.example.com", "tls": true
            });
            let sb = singbox_outbound_from_clash(&payload, "x")
                .unwrap_or_else(|| panic!("{proto} should be supported"));
            assert!(
                sb.get("tls").is_none(),
                "{proto} has no tls block in sing-box — it would be rejected"
            );
        }
    }

    #[test]
    fn singbox_skips_protocols_it_cannot_represent() {
        // mieru / sudoku / ssr: not in sing-box at all.
        // shadowtls: a TLS wrapper, not a standalone outbound.
        // wireguard: the outbound was REMOVED in sing-box 1.13 (endpoint form only).
        for proto in ["mieru", "sudoku", "ssr", "shadowtls", "wireguard"] {
            let payload = serde_json::json!({
                "name": "x", "type": proto, "server": "x.example.com", "port": 1080,
                "password": "pw", "private-key": "k"
            });
            assert!(
                singbox_outbound_from_clash(&payload, "x").is_none(),
                "{proto} must be skipped for the sing-box target"
            );
            assert!(!singbox_supports(proto));
        }
        assert!(singbox_supports("vmess"));
        assert!(singbox_supports("hysteria2"));
    }

    #[test]
    fn singbox_maps_snell_obfs() {
        let payload = serde_json::json!({
            "name": "n", "type": "snell", "server": "n.example.com", "port": 443,
            "psk": "psk", "version": 4, "obfs-opts": {"mode": "http", "host": "bing.com"}
        });
        let sb = singbox_outbound_from_clash(&payload, "n").expect("snell");
        assert_eq!(sb["psk"], "psk");
        assert_eq!(sb["version"], 4);
        assert_eq!(sb["obfs_mode"], "http");
        assert_eq!(sb["obfs_host"], "bing.com");
    }

    #[test]
    fn singbox_maps_socks5_to_socks_with_version() {
        let payload = serde_json::json!({
            "name": "s", "type": "socks5", "server": "s.example.com", "port": 1080,
            "username": "u", "password": "pw"
        });
        let sb = singbox_outbound_from_clash(&payload, "s").expect("socks5");
        assert_eq!(sb["type"], "socks");
        assert_eq!(sb["version"], "5");
        assert_eq!(sb["username"], "u");
    }

    /// sing-box's `parsePorts` requires a **colon in every** `server_ports`
    /// element (sing-quic `hysteria2/client.go`: `if !strings.Contains(portRange, ":")`),
    /// so a bare single port has to become `N:N`. Verified against the real
    /// sing-box v1.14.4 binary: `["60000"]` -> "bad port range: 60000".
    #[test]
    fn singbox_port_ranges_always_carry_a_colon() {
        let cases: &[(&str, serde_json::Value)] = &[
            ("60000-65530", serde_json::json!(["60000:65530"])),
            ("443", serde_json::json!(["443:443"])),
            ("443,8443-8444", serde_json::json!(["443:443", "8443:8444"])),
            ("60000:", serde_json::json!(["60000:"])),
        ];
        for (input, expected) in cases {
            let payload = serde_json::json!({
                "name": "h", "type": "hysteria2", "server": "h.example.com",
                "port": 443, "password": "pw", "ports": input
            });
            let sb = singbox_outbound_from_clash(&payload, "h").expect("hysteria2");
            assert_eq!(&sb["server_ports"], expected, "input was {input}");
        }
    }
}
