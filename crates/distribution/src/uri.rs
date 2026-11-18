//! Clash proxy payload -> share-link URI (M1c).
//!
//! The subscription pipeline's `base64` format needs real URIs; before this module it
//! emitted fabricated links. Only protocols whose URI form is unambiguous here are
//! supported — anything else returns `None` and is omitted from the output rather than
//! being emitted as a link that will not connect.
use base64::Engine;
use serde_json::{json, Value};

/// RFC 3986 unreserved characters stay literal; everything else is percent-encoded.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn str_of<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn bool_of(p: &Value, key: &str) -> bool {
    p.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn port_of(p: &Value) -> Option<u16> {
    p.get("port")
        .and_then(|v| v.as_u64())
        .and_then(|v| u16::try_from(v).ok())
}

/// Host part: IPv6 literals must be bracketed inside a URI authority.
fn host_of(server: &str) -> String {
    if server.contains(':') && !server.starts_with('[') {
        format!("[{server}]")
    } else {
        server.to_string()
    }
}

/// Query string assembly that skips empty values (`k=&` noise).
fn query(pairs: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| {
            let v = v.as_ref()?;
            if v.is_empty() {
                return None;
            }
            Some(format!("{k}={}", pct(v)))
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

fn ws_path(p: &Value) -> Option<String> {
    p.get("ws-opts")
        .and_then(|v| v.get("path"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn ws_host(p: &Value) -> Option<String> {
    p.get("ws-opts")
        .and_then(|v| v.get("headers"))
        .and_then(|v| v.get("Host").or_else(|| v.get("host")))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn network(p: &Value) -> Option<String> {
    p.get("network")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && *s != "tcp")
        .map(str::to_string)
}

fn sni(p: &Value) -> Option<String> {
    str_of(p, "servername")
        .or_else(|| str_of(p, "sni"))
        .map(str::to_string)
}

/// Build the share link for one Clash-shaped payload.
///
/// Returns `None` for protocols without a supported URI form (mieru, sudoku, ssr,
/// shadowtls, wireguard) and for shapes whose URI mapping would be guesswork.
pub fn uri_from_clash(p: &Value) -> Option<String> {
    let ty = str_of(p, "type")?;
    let server = str_of(p, "server")?;
    let port = port_of(p)?;
    let host = host_of(server);
    let name = str_of(p, "name").unwrap_or("");
    let tag = if name.is_empty() {
        String::new()
    } else {
        format!("#{}", pct(name))
    };
    let authority = format!("{host}:{port}");

    match ty {
        // SIP002: ss://base64url(method:password)@host:port#tag
        "ss" | "shadowsocks" => {
            // A plugin changes the URI enough that guessing would be unsafe.
            if p.get("plugin").is_some() {
                return None;
            }
            let cipher = str_of(p, "cipher").or_else(|| str_of(p, "method"))?;
            let password = str_of(p, "password")?;
            let user = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!("{cipher}:{password}"));
            Some(format!("ss://{user}@{authority}{tag}"))
        }
        "vmess" => {
            let uuid = str_of(p, "uuid")?;
            let net = str_of(p, "network").unwrap_or("tcp");
            let tls_on = bool_of(p, "tls");
            let alter = p
                .get("alterId")
                .or_else(|| p.get("alter-id"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let body = json!({
                "v": "2",
                "ps": name,
                "add": server,
                "port": port.to_string(),
                "id": uuid,
                "aid": alter.to_string(),
                "scy": str_of(p, "cipher").unwrap_or("auto"),
                "net": net,
                "type": "none",
                "host": ws_host(p).unwrap_or_default(),
                "path": ws_path(p).unwrap_or_default(),
                "tls": if tls_on { "tls" } else { "" },
                "sni": sni(p).unwrap_or_default(),
                "alpn": "",
                "fp": str_of(p, "client-fingerprint").unwrap_or_default(),
            });
            let encoded = base64::engine::general_purpose::STANDARD.encode(body.to_string());
            Some(format!("vmess://{encoded}"))
        }
        "vless" => {
            let uuid = str_of(p, "uuid")?;
            let tls_on = bool_of(p, "tls");
            let reality = p.get("reality-opts").is_some();
            let security = if reality {
                "reality"
            } else if tls_on {
                "tls"
            } else {
                "none"
            };
            let (pbk, sid) = p
                .get("reality-opts")
                .map(|r| {
                    (
                        r.get("public-key")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        r.get("short-id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    )
                })
                .unwrap_or((None, None));
            let q = query(&[
                ("encryption", Some("none".into())),
                ("security", Some(security.into())),
                ("sni", sni(p)),
                ("fp", str_of(p, "client-fingerprint").map(str::to_string)),
                ("pbk", pbk),
                ("sid", sid),
                ("flow", str_of(p, "flow").map(str::to_string)),
                ("type", network(p)),
                ("path", ws_path(p)),
                ("host", ws_host(p)),
            ]);
            Some(format!("vless://{uuid}@{authority}{q}{tag}"))
        }
        "trojan" => {
            let password = str_of(p, "password")?;
            let q = query(&[
                ("security", Some("tls".into())),
                ("sni", sni(p)),
                ("type", network(p)),
                ("path", ws_path(p)),
                ("host", ws_host(p)),
                ("alpn", str_of(p, "alpn").map(str::to_string)),
            ]);
            Some(format!("trojan://{}@{authority}{q}{tag}", pct(password)))
        }
        "hysteria2" | "hy2" => {
            let password = str_of(p, "password")?;
            let q = query(&[
                ("sni", sni(p)),
                (
                    "insecure",
                    bool_of(p, "skip-cert-verify").then(|| "1".to_string()),
                ),
                ("obfs", str_of(p, "obfs").map(str::to_string)),
                (
                    "obfs-password",
                    str_of(p, "obfs-password").map(str::to_string),
                ),
            ]);
            Some(format!("hysteria2://{}@{authority}{q}{tag}", pct(password)))
        }
        "tuic" => {
            let uuid = str_of(p, "uuid")?;
            let password = str_of(p, "password")?;
            let q = query(&[
                ("sni", sni(p)),
                (
                    "congestion_control",
                    str_of(p, "congestion-controller").map(str::to_string),
                ),
                (
                    "udp_relay_mode",
                    str_of(p, "udp-relay-mode").map(str::to_string),
                ),
                ("alpn", str_of(p, "alpn").map(str::to_string)),
                (
                    "allow_insecure",
                    bool_of(p, "skip-cert-verify").then(|| "1".to_string()),
                ),
            ]);
            Some(format!(
                "tuic://{}:{}@{authority}{q}{tag}",
                pct(uuid),
                pct(password)
            ))
        }
        "socks5" => {
            let user = str_of(p, "username").unwrap_or("");
            let pass = str_of(p, "password").unwrap_or("");
            let creds = if user.is_empty() && pass.is_empty() {
                String::new()
            } else {
                format!("{}:{}@", pct(user), pct(pass))
            };
            Some(format!("socks5://{creds}{authority}{tag}"))
        }
        "http" => {
            let user = str_of(p, "username").unwrap_or("");
            let pass = str_of(p, "password").unwrap_or("");
            let creds = if user.is_empty() && pass.is_empty() {
                String::new()
            } else {
                format!("{}:{}@", pct(user), pct(pass))
            };
            Some(format!("http://{creds}{authority}{tag}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ss_uses_sip002_userinfo() {
        let p = json!({"type":"ss","server":"a.example","port":8388,"cipher":"aes-256-gcm","password":"p@ss","name":"JP 01"});
        let uri = uri_from_clash(&p).unwrap();
        assert!(uri.starts_with("ss://"), "{uri}");
        let user = uri["ss://".len()..].split('@').next().unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(user)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "aes-256-gcm:p@ss");
        assert!(uri.ends_with("#JP%2001"), "{uri}");
    }

    #[test]
    fn vmess_is_base64_json() {
        let p = json!({"type":"vmess","server":"b.example","port":443,"uuid":"11111111-1111-1111-1111-111111111111","alterId":0,"cipher":"auto","network":"ws","tls":true,"servername":"b.example","ws-opts":{"path":"/ray","headers":{"Host":"b.example"}},"name":"US 02"});
        let uri = uri_from_clash(&p).unwrap();
        let body = uri.strip_prefix("vmess://").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body)
            .unwrap();
        let v: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["add"], "b.example");
        assert_eq!(v["id"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(v["net"], "ws");
        assert_eq!(v["tls"], "tls");
        assert_eq!(v["ps"], "US 02");
    }

    #[test]
    fn vless_carries_security_and_reality() {
        let p = json!({"type":"vless","server":"c.example","port":443,"uuid":"22222222-2222-2222-2222-222222222222","tls":true,"servername":"c.example","client-fingerprint":"chrome","flow":"xtls-rprx-vision","network":"ws","ws-opts":{"path":"/ws","headers":{"Host":"cdn.example"}},"name":"HK"});
        let uri = uri_from_clash(&p).unwrap();
        assert!(
            uri.starts_with("vless://22222222-2222-2222-2222-222222222222@c.example:443?"),
            "{uri}"
        );
        assert!(uri.contains("security=tls"), "{uri}");
        assert!(uri.contains("flow=xtls-rprx-vision"), "{uri}");
        assert!(uri.contains("fp=chrome"), "{uri}");
        assert!(uri.contains("type=ws"), "{uri}");
        assert!(uri.contains("path=%2Fws"), "{uri}");
        assert!(uri.contains("host=cdn.example"), "{uri}");
        assert!(uri.ends_with("#HK"), "{uri}");

        let reality = json!({"type":"vless","server":"d.example","port":443,"uuid":"33333333-3333-3333-3333-333333333333","tls":true,"servername":"d.example","client-fingerprint":"chrome","reality-opts":{"public-key":"PBK","short-id":"ab"},"name":"R"});
        let uri = uri_from_clash(&reality).unwrap();
        assert!(uri.contains("security=reality"), "{uri}");
        assert!(uri.contains("pbk=PBK"), "{uri}");
        assert!(uri.contains("sid=ab"), "{uri}");
    }

    #[test]
    fn trojan_hysteria2_tuic() {
        let trojan = uri_from_clash(&json!({"type":"trojan","server":"e.example","port":443,"password":"p@ss w","servername":"e.example","name":"T"})).unwrap();
        assert!(
            trojan.starts_with("trojan://p%40ss%20w@e.example:443?"),
            "{trojan}"
        );
        assert!(trojan.contains("security=tls"), "{trojan}");

        let hy2 = uri_from_clash(&json!({"type":"hysteria2","server":"f.example","port":8443,"password":"pw","servername":"f.example","obfs":"salamander","obfs-password":"op","skip-cert-verify":true,"name":"H"})).unwrap();
        assert!(hy2.starts_with("hysteria2://pw@f.example:8443?"), "{hy2}");
        assert!(hy2.contains("obfs=salamander"), "{hy2}");
        assert!(hy2.contains("insecure=1"), "{hy2}");

        let tuic = uri_from_clash(&json!({"type":"tuic","server":"g.example","port":443,"uuid":"44444444-4444-4444-4444-444444444444","password":"tp","servername":"g.example","congestion-controller":"bbr","name":"U"})).unwrap();
        assert!(
            tuic.starts_with("tuic://44444444-4444-4444-4444-444444444444:tp@g.example:443?"),
            "{tuic}"
        );
        assert!(tuic.contains("congestion_control=bbr"), "{tuic}");
    }

    #[test]
    fn unsupported_protocols_yield_none() {
        for ty in ["mieru", "sudoku", "ssr", "shadowtls", "wireguard"] {
            assert!(
                uri_from_clash(&json!({"type":ty,"server":"x","port":1})).is_none(),
                "{ty}"
            );
        }
        // ss with a plugin: the URI form would be guesswork.
        assert!(uri_from_clash(&json!({"type":"ss","server":"x","port":1,"cipher":"aes-256-gcm","password":"p","plugin":"obfs"})).is_none());
    }

    #[test]
    fn ipv6_host_is_bracketed() {
        let uri = uri_from_clash(
            &json!({"type":"trojan","server":"2001:db8::1","port":443,"password":"p","name":"v6"}),
        )
        .unwrap();
        assert!(uri.contains("@[2001:db8::1]:443"), "{uri}");
    }
}
