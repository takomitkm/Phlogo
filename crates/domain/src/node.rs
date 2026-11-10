//! Normalized node model (plan §7.2). Identity fields vs display/source-tracking fields are
//! strictly separated: only identity fields feed `identity_material()` (→ fingerprint, p2-03).
use serde::{Deserialize, Serialize};

/// Primary credential per protocol (mirrors Phlogo `nodeAuth`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Credential {
    Uuid(String),
    Password(String),
    None,
}

/// Transport layer settings that affect connection identity (ws/grpc/h2…).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Transport {
    pub kind: String,
    pub path: Option<String>,
    pub host: Option<String>,
    pub service_name: Option<String>,
}

/// TLS-ish settings that affect connection identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TlsSettings {
    pub enabled: bool,
    pub sni: Option<String>,
    pub alpn: Vec<String>,
    /// REALITY public key (if present).
    pub reality: Option<String>,
}

/// Which source / fetch run produced this node (display/tracking only; NOT identity).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceContext {
    pub source_id: uuid::Uuid,
    pub fetch_run_id: uuid::Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedNode {
    // ---- identity fields ----
    pub protocol: String,
    pub server: String,
    pub port: u16,
    pub credentials: Credential,
    pub transport: Option<Transport>,
    pub tls: Option<TlsSettings>,
    pub network_options: Option<serde_json::Value>,

    // ---- display / source tracking (NOT identity) ----
    pub display_name: Option<String>,
    pub source_context: SourceContext,
    pub raw_config_reference: String,

    // ---- filled by p2-03 ----
    pub identity_hash: Option<String>,
    pub fingerprint_version: Option<u8>,
}

#[derive(Serialize)]
struct IdentityMaterial<'a> {
    protocol: &'a str,
    server: &'a str,
    port: u16,
    credentials: &'a Credential,
    transport: &'a Option<Transport>,
    tls: &'a Option<TlsSettings>,
    network_options: &'a Option<serde_json::Value>,
}

impl NormalizedNode {
    /// Canonical identity material: ONLY identity fields (display_name/source_context/raw excluded).
    /// p2-03 hashes this (with a fingerprint_version) to derive `identity_hash`.
    pub fn identity_material(&self) -> serde_json::Value {
        serde_json::to_value(IdentityMaterial {
            protocol: &self.protocol,
            server: &self.server,
            port: self.port,
            credentials: &self.credentials,
            transport: &self.transport,
            tls: &self.tls,
            network_options: &self.network_options,
        })
        .expect("identity material serializes")
    }
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

    fn node(name: &str) -> NormalizedNode {
        NormalizedNode {
            protocol: "trojan".into(),
            server: "a.example.com".into(),
            port: 443,
            credentials: Credential::Password("pw".into()),
            transport: Some(Transport {
                kind: "ws".into(),
                path: Some("/x".into()),
                host: None,
                service_name: None,
            }),
            tls: Some(TlsSettings {
                enabled: true,
                sni: Some("a.example.com".into()),
                alpn: vec![],
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

    #[test]
    fn identity_ignores_display_name_but_not_transport() {
        let a = node("name-a");
        let mut b = node("name-b");
        assert_eq!(
            a.identity_material(),
            b.identity_material(),
            "renaming must not change identity"
        );
        b.transport.as_mut().unwrap().path = Some("/y".into());
        assert_ne!(
            a.identity_material(),
            b.identity_material(),
            "transport change must change identity"
        );
    }
}
