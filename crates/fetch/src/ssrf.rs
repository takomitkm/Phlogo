//! SSRF guard: block loopback/private/link-local/ULA/metadata targets.
//! Mirrors plan §16.9. Hostnames are resolved and EVERY resolved IP must pass.
use ipnetwork::IpNetwork;
use std::net::IpAddr;
use std::sync::OnceLock;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SsrfError {
    #[error("blocked address: {0}")]
    BlockedAddress(IpAddr),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("dns resolution failed: {0}")]
    DnsResolution(String),
}

const BLOCKED: &[&str] = &[
    "0.0.0.0/8",
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "192.0.0.170/32",
    "::/128",
    "::1/128",
    "fe80::/10",
    "fc00::/7",
    "::ffff:0:0/96",
];

fn blocked() -> &'static Vec<IpNetwork> {
    static B: OnceLock<Vec<IpNetwork>> = OnceLock::new();
    B.get_or_init(|| {
        BLOCKED
            .iter()
            .map(|s| s.parse::<IpNetwork>().expect("valid CIDR"))
            .collect()
    })
}

/// Allowlist check (e.g. tests pinning a loopback mock server).
pub fn is_allowed_by(ip: IpAddr, allow: &[IpNetwork]) -> bool {
    allow.iter().any(|n| n.contains(ip))
}

/// Check a single resolved IP against the blocklist.
pub fn check_ip(ip: IpAddr) -> Result<(), SsrfError> {
    for net in blocked() {
        if net.contains(ip) {
            return Err(SsrfError::BlockedAddress(ip));
        }
    }
    Ok(())
}

/// Resolve the URL host and verify every address (literal IPs are checked directly).
pub async fn guard(url: &str, allow: &[IpNetwork]) -> Result<(), SsrfError> {
    let parsed = reqwest::Url::parse(url).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| SsrfError::InvalidUrl("missing host".into()))?;

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_allowed_by(ip, allow) {
            return Ok(());
        }
        return check_ip(ip);
    }

    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| SsrfError::InvalidUrl("missing port".into()))?;
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| SsrfError::DnsResolution(e.to_string()))?;
    let mut any = false;
    for a in addrs {
        any = true;
        if is_allowed_by(a.ip(), allow) {
            continue;
        }
        check_ip(a.ip())?;
    }
    if !any {
        return Err(SsrfError::DnsResolution("no addresses".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_dangerous_and_allows_public() {
        for bad in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.3.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "fe80::1",
            "192.0.0.170",
        ] {
            assert!(
                matches!(check_ip(ip(bad)), Err(SsrfError::BlockedAddress(_))),
                "expected {bad} blocked"
            );
        }
        for good in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(check_ip(ip(good)).is_ok(), "expected {good} allowed");
        }
    }
}
