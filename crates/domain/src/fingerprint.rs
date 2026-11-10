//! Node fingerprint (plan §7.3; ports Phlogo `internal/node/upsert.go`):
//! `nodeAuth` (per-protocol primary credential) + `normalizeCredential`
//! (repeated percent-decode until stable) + SHA-256 over the canonical string.
//!
//! Deviation from Phlogo, documented: Phlogo's `nodeHash` canonical is
//! `type|server|port|auth` only. Our `fingerprint_version = 1` canonical also
//! includes transport / tls / network_options, because plan §7.3 requires
//! connection-affecting parameters to participate in identity. A Phlogo-compat
//! canonical (without transport/tls) would be version 0 and is not produced here.
use crate::node::{Credential, NormalizedNode, TlsSettings, Transport};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};

pub const CURRENT_FINGERPRINT_VERSION: u8 = 1;

/// Repeated percent-decode until stable (max 16 rounds). Prevents multi-level
/// `%25` escapes from producing different hashes. Mirrors Go `normalizeCredential`:
/// on an invalid escape (e.g. `%de` -> non-UTF-8 byte, Go's PathUnescape errors)
/// the string is kept as-is instead of being lossily mangled.
pub fn normalize_credential(s: &str) -> String {
    let mut cur = s.to_string();
    for _ in 0..16 {
        let decoded = match percent_decode_str(&cur).decode_utf8() {
            Ok(d) => d.into_owned(),
            Err(_) => break,
        };
        if decoded == cur {
            break;
        }
        cur = decoded;
    }
    cur
}

fn password_of(cred: &Credential) -> Option<&str> {
    match cred {
        Credential::Password(p) => Some(p),
        Credential::UsernamePassword { password, .. } => Some(password),
        Credential::Tuic { password, .. } => Some(password),
        _ => None,
    }
}

/// Per-protocol primary credential (mirrors Go `nodeAuth` switch).
pub fn node_auth(ptype: &str, cred: &Credential) -> String {
    match ptype {
        "vmess" | "vless" => match cred {
            Credential::Uuid(u) => u.clone(),
            _ => String::new(),
        },
        "trojan" | "anytls" | "hysteria" | "hysteria2" | "ss" | "ssr" => {
            password_of(cred).unwrap_or_default().to_string()
        }
        "tuic" => match cred {
            Credential::Tuic { uuid, password } if !uuid.is_empty() => {
                format!("{uuid}:{password}")
            }
            Credential::Tuic { password, .. } => password.clone(),
            Credential::Password(p) => p.clone(),
            _ => String::new(),
        },
        "wireguard" => match cred {
            Credential::PrivateKey(k) => k.clone(),
            _ => String::new(),
        },
        "socks5" | "http" | "mieru" => match cred {
            Credential::UsernamePassword { username, password } => {
                format!("{username}:{password}")
            }
            Credential::Password(p) => format!(":{p}"),
            _ => String::new(),
        },
        "snell" => match cred {
            Credential::Psk(p) => p.clone(),
            _ => String::new(),
        },
        "sudoku" => match cred {
            Credential::Key(k) => k.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// Empty/default transport normalizes to `-` (null_default invariant).
fn canon_transport(t: &Option<Transport>) -> String {
    match t {
        None => "-".to_string(),
        Some(t) => {
            if t.kind.trim().is_empty()
                && t.path.is_none()
                && t.host.is_none()
                && t.service_name.is_none()
            {
                return "-".to_string();
            }
            format!(
                "t:{}|{}|{}|{}",
                t.kind.trim(),
                t.path.clone().unwrap_or_default(),
                t.host.clone().unwrap_or_default(),
                t.service_name.clone().unwrap_or_default()
            )
        }
    }
}

/// Empty/default tls (disabled, no sni/alpn/reality) normalizes to `-`.
fn canon_tls(t: &Option<TlsSettings>) -> String {
    match t {
        None => "-".to_string(),
        Some(t) => {
            if !t.enabled && t.sni.is_none() && t.alpn.is_empty() && t.reality.is_none() {
                return "-".to_string();
            }
            format!(
                "s:{}|{}|{}|{}",
                t.enabled,
                t.sni.clone().unwrap_or_default(),
                t.alpn.join(","),
                t.reality.clone().unwrap_or_default()
            )
        }
    }
}

fn canon_net(v: &Option<serde_json::Value>) -> String {
    match v {
        None => "-".to_string(),
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Compute `identity_hash`; `None` when protocol/server/port are empty (Go returns "").
pub fn compute_identity(nn: &NormalizedNode) -> Option<String> {
    if nn.protocol.is_empty() || nn.server.is_empty() || nn.port == 0 {
        return None;
    }
    let auth = normalize_credential(&node_auth(&nn.protocol, &nn.credentials));
    let canonical = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        nn.protocol,
        nn.server,
        nn.port,
        auth,
        canon_transport(&nn.transport),
        canon_tls(&nn.tls),
        canon_net(&nn.network_options)
    );
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    let sum = h.finalize();
    Some(hex::encode(&sum[..16]))
}

/// Fill `identity_hash` + `fingerprint_version` on the node.
pub fn stamp(nn: &mut NormalizedNode) {
    if let Some(hash) = compute_identity(nn) {
        nn.identity_hash = Some(hash);
        nn.fingerprint_version = Some(CURRENT_FINGERPRINT_VERSION);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::SourceContext;
    use uuid::Uuid;

    fn ctx() -> SourceContext {
        SourceContext {
            source_id: Uuid::nil(),
            fetch_run_id: Uuid::nil(),
        }
    }

    fn trojan(name: &str) -> NormalizedNode {
        NormalizedNode {
            protocol: "trojan".into(),
            server: "a.example.com".into(),
            port: 443,
            credentials: Credential::Password("s3cr3t".into()),
            transport: Some(Transport {
                kind: "ws".into(),
                path: Some("/ray".into()),
                host: Some("a.example.com".into()),
                service_name: None,
            }),
            tls: Some(TlsSettings {
                enabled: true,
                sni: Some("a.example.com".into()),
                alpn: vec!["h2".into()],
                reality: None,
            }),
            network_options: None,
            display_name: Some(name.into()),
            source_context: ctx(),
            raw_config_reference: "{}".into(),
            identity_hash: None,
            fingerprint_version: None,
        }
    }

    fn vmess(uuid: &str) -> NormalizedNode {
        let mut n = trojan("x");
        n.protocol = "vmess".into();
        n.credentials = Credential::Uuid(uuid.into());
        n
    }

    #[test]
    fn name_change_same_identity() {
        let a = trojan("name-a");
        let b = trojan("name-b");
        assert_eq!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn uuid_change_differs() {
        let a = vmess("11111111-1111-1111-1111-111111111111");
        let b = vmess("22222222-2222-2222-2222-222222222222");
        assert_ne!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn password_change_differs() {
        let a = trojan("a");
        let mut b = trojan("a");
        b.credentials = Credential::Password("other".into());
        assert_ne!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn transport_change_differs() {
        let a = trojan("a");
        let mut b = trojan("a");
        b.transport.as_mut().unwrap().path = Some("/other".into());
        assert_ne!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn tls_change_differs() {
        let a = trojan("a");
        let mut b = trojan("a");
        b.tls.as_mut().unwrap().sni = Some("other.example.com".into());
        assert_ne!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn server_change_differs() {
        let a = trojan("a");
        let mut b = trojan("a");
        b.server = "b.example.com".into();
        assert_ne!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn port_change_differs() {
        let a = trojan("a");
        let mut b = trojan("a");
        b.port = 8443;
        assert_ne!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn field_order_same_identity() {
        let mut a = trojan("a");
        let mut b = trojan("a");
        // serde_json objects are order-insensitive (BTreeMap), so different key
        // orders normalize to the same value and the same hash.
        a.network_options = Some(serde_json::json!({"z": 1, "a": 2}));
        b.network_options = Some(serde_json::json!({"a": 2, "z": 1}));
        assert_eq!(compute_identity(&a), compute_identity(&b));
    }

    #[test]
    fn null_default_same_identity() {
        let mut a = trojan("a");
        a.transport = None;
        a.tls = None;
        let mut b = trojan("a");
        b.transport = Some(Transport::default());
        b.tls = Some(TlsSettings::default());
        assert_eq!(
            compute_identity(&a),
            compute_identity(&b),
            "absent vs empty-default transport/tls must hash identically"
        );
    }

    #[test]
    fn percent_decode_invariant() {
        assert_eq!(normalize_credential("abc%25def"), "abc%def");
        assert_eq!(normalize_credential("abc%def"), "abc%def");
        assert_eq!(normalize_credential("abc%20def"), "abc def");
        assert_eq!(normalize_credential("abc def"), "abc def");
        assert_eq!(
            normalize_credential("abc%2520def"),
            normalize_credential("abc%20def"),
            "multi-level %25 vs single-level decode converge"
        );
    }

    #[test]
    fn percent_decode_feeds_identity() {
        let mut a = trojan("a");
        a.credentials = Credential::Password("abc%25def".into());
        let mut b = trojan("a");
        b.credentials = Credential::Password("abc%def".into());
        assert_eq!(
            compute_identity(&a),
            compute_identity(&b),
            "encoded vs literal credential must hash identically"
        );
    }

    #[test]
    fn empty_identity_none() {
        let mut n = trojan("a");
        n.protocol = String::new();
        assert!(compute_identity(&n).is_none());
        let mut n = trojan("a");
        n.server = String::new();
        assert!(compute_identity(&n).is_none());
        let mut n = trojan("a");
        n.port = 0;
        assert!(compute_identity(&n).is_none());
    }

    #[test]
    fn stamp_sets_version() {
        let mut n = trojan("a");
        stamp(&mut n);
        assert_eq!(n.fingerprint_version, Some(CURRENT_FINGERPRINT_VERSION));
        assert!(n.identity_hash.is_some());
        assert_eq!(n.identity_hash.as_deref().map(str::len), Some(32));
    }
}
