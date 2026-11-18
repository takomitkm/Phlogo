//! Subscription pipeline (P5-03): token -> profile -> filter AST -> preset selection
//! -> naming -> output rendering. Pure in-memory form; DB/HTTP assembly lands in P6.
use filter::{matches, parse, NodeView};
use serde_json::Value;

use crate::naming::{fields_for, render, GradeThresholds};
use crate::output::{
    base64_subs, clash_proxy_from, mihomo_yaml, singbox_json, singbox_outbound_from_clash,
    OutputNode,
};
use crate::{select_with_preset, Candidate};

#[derive(Debug, Clone, Default)]
pub struct ProfileSpec {
    /// AST JSON string; empty = no filtering.
    pub filter_ast: String,
    pub region: Option<String>,
    /// The profile's **desired** health as a hard condition (like region/min_score):
    /// only nodes whose actual health equals or is better than this pass.
    /// `""` / `"any"` disables the condition. It is NOT fed to the AST as node state.
    pub health: String,
    pub min_score: f64,
    pub limit_count: usize,
    pub preset: String,
    pub naming_rule: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PipelineNode {
    pub id: String,
    pub region: String,
    pub score: f64,
    /// **The node's actual** health (alive | degraded | dead | unknown), merged across
    /// checker tags. Filter ASTs see this — never the profile's *desired* health,
    /// which is a separate hard condition (`ProfileSpec.health`).
    pub health: String,
    /// Clash-shaped proxy payload from `NormalizedNode`.
    pub clash_proxy: Value,
    pub protocol: String,
    pub server: String,
    pub port: u16,
    pub credential: String,
    pub tls_sni: Option<String>,
    pub ws_path: Option<String>,
    pub streaming: std::collections::HashMap<String, bool>,
}

pub enum RenderedSubscription {
    Mihomo(String),
    Singbox(String),
    Base64(String),
}

/// Eval-side view for the filter AST: the node's ACTUAL health (from `node_status`),
/// so `{"field":"health",...}` conditions filter on reality, not on the profile's wish.
fn to_node_view(n: &PipelineNode) -> NodeView {
    NodeView {
        health: n.health.clone(),
        region: Some(n.region.clone()),
        score: n.score,
        protocol: n.protocol.clone(),
        server: n.server.clone(),
        source_id: None,
        streaming: n.streaming.clone(),
    }
}

/// Rank of a health value for the `ProfileSpec.health` hard condition (higher = better).
/// `unknown` is treated as worse than dead: never served when a health bar is set.
fn health_rank(h: &str) -> i32 {
    match h {
        "alive" => 3,
        "degraded" => 2,
        "dead" => 1,
        _ => 0,
    }
}

/// `true` = the node passes the profile's desired-health bar.
/// `""`/`"any"` disables the condition (the AST may still filter on real health).
fn passes_health_bar(actual: &str, desired: &str) -> bool {
    let d = desired.trim();
    if d.is_empty() || d.eq_ignore_ascii_case("any") {
        return true;
    }
    health_rank(actual) >= health_rank(d)
}

/// Run the full subscription pipeline.
pub fn render_subscription(
    nodes: &[PipelineNode],
    profile: &ProfileSpec,
    preset_override: Option<&str>,
    max_distribute: usize,
    thresholds: &GradeThresholds,
    unlock_flags: &str,
    format: &str,
) -> Result<RenderedSubscription, String> {
    render_subscription_with_sponsor(
        nodes,
        profile,
        preset_override,
        max_distribute,
        thresholds,
        unlock_flags,
        format,
        None,
    )
}

/// Like [`render_subscription`] but with a per-node sponsor text resolver
/// (B3 gap #2: the upstream sponsor match, wired into naming).
#[allow(clippy::too_many_arguments)]
pub fn render_subscription_with_sponsor(
    nodes: &[PipelineNode],
    profile: &ProfileSpec,
    preset_override: Option<&str>,
    max_distribute: usize,
    thresholds: &GradeThresholds,
    unlock_flags: &str,
    format: &str,
    sponsor: Option<&dyn Fn(&PipelineNode) -> String>,
) -> Result<RenderedSubscription, String> {
    // 1) Hard conditions + AST filter (profile.filter_ast + region/min_score).
    let ast = if profile.filter_ast.trim().is_empty() {
        None
    } else {
        Some(parse(&profile.filter_ast).map_err(|e| format!("profile filter: {e}"))?)
    };
    let candidates: Vec<Candidate> = nodes
        .iter()
        .filter(|n| {
            if let Some(want) = &profile.region {
                if !n.region.eq_ignore_ascii_case(want) {
                    return false;
                }
            }
            if !passes_health_bar(&n.health, &profile.health) {
                return false;
            }
            if n.score < profile.min_score {
                return false;
            }
            if let Some(ast) = &ast {
                if !matches(&to_node_view(n), ast) {
                    return false;
                }
            }
            true
        })
        .map(|n| Candidate {
            id: n.id.clone(),
            region: n.region.clone(),
            score: n.score,
        })
        .collect();

    // 2) Preset: URL override > profile preset (tag/global defaults folded upstream).
    let preset = preset_override.unwrap_or(&profile.preset);
    let limit = profile.limit_count.min(max_distribute);
    let mut rng = {
        use rand::SeedableRng;
        rand::rngs::StdRng::from_entropy()
    };
    let selected = select_with_preset(&candidates, limit, preset, &mut rng);
    if selected.is_empty() {
        return Err("no nodes matched the profile".into());
    }

    // 3) Name + build outputs. Sponsor text is per-node (upstream `Service::Match`):
    // the caller supplies a resolver so this pure function stays DB-free.
    let mut outputs = Vec::with_capacity(selected.len());
    for sel in &selected {
        let node = nodes
            .iter()
            .find(|n| n.id == sel.candidate.id)
            .ok_or("selected node vanished")?;
        let mut f = fields_for(sel, thresholds, unlock_flags);
        if let Some(resolve) = sponsor {
            f.sponsor = resolve(node);
        }
        let name = match &profile.naming_rule {
            Some(t) => render(t, &f),
            None => render(crate::naming::DEFAULT_TEMPLATE, &f),
        };
        outputs.push(OutputNode {
            selected: sel.clone(),
            clash_proxy: clash_proxy_from(node.clash_proxy.clone(), &name),
            // Full-fidelity sing-box conversion from the Clash-shaped payload, so
            // protocol-specific fields (hysteria2 password/obfs/ports, tuic
            // uuid+password, shadowsocks method, …) survive — not just
            // vmess/vless/trojan. Protocols sing-box cannot represent (mieru,
            // sudoku, ssr, shadowtls) are skipped for the sing-box target.
            singbox_outbound: singbox_outbound_from_clash(&node.clash_proxy, &name)
                .unwrap_or_else(|| serde_json::json!({"tag": name, "type": "direct"})),
            name,
        });
    }

    // 4) Render in the requested format.
    match format {
        "yaml" | "mihomo" | "clash" => Ok(RenderedSubscription::Mihomo(mihomo_yaml(
            crate::output::DEFAULT_CLASH_TEMPLATE,
            &outputs,
        ))),
        "json" | "singbox" => Ok(RenderedSubscription::Singbox(singbox_json(
            crate::output::DEFAULT_SINGBOX_TEMPLATE,
            &outputs,
        ))),
        "base64" => {
            // Real share links (M1c). Protocols without a supported URI form are
            // omitted rather than emitted as links that would not connect.
            let links: Vec<String> = outputs
                .iter()
                .filter_map(|o| crate::uri::uri_from_clash(&o.clash_proxy))
                .collect();
            if links.is_empty() {
                return Err("no URI-representable nodes for base64".into());
            }
            Ok(RenderedSubscription::Base64(base64_subs(&links)))
        }
        other => Err(format!("unknown format: {other}")),
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::output::DEFAULT_SINGBOX_TEMPLATE;

    fn sample_nodes() -> Vec<PipelineNode> {
        let mut streaming = std::collections::HashMap::new();
        streaming.insert("netflix".to_string(), true);
        vec![
            PipelineNode {
                id: "n1".into(),
                region: "JP".into(),
                score: 0.9,
                health: "alive".into(),
                clash_proxy: serde_json::json!({"type":"trojan","server":"a.jp","port":443,"password":"pw"}),
                protocol: "trojan".into(),
                server: "a.jp".into(),
                port: 443,
                credential: "pw".into(),
                tls_sni: Some("a.jp".into()),
                ws_path: None,
                streaming: streaming.clone(),
            },
            PipelineNode {
                id: "n2".into(),
                region: "US".into(),
                score: 0.4,
                health: "alive".into(),
                clash_proxy: serde_json::json!({"type":"trojan","server":"a.us","port":443,"password":"pw"}),
                protocol: "trojan".into(),
                server: "a.us".into(),
                port: 443,
                credential: "pw".into(),
                tls_sni: None,
                ws_path: None,
                streaming,
            },
        ]
    }

    /// The profile's health bar must gate on the node's ACTUAL health
    /// (alive > degraded > dead > unknown), and AST health conditions must see the real
    /// state, not the profile's wish (regression for the S64 review finding).
    #[test]
    fn health_bar_gates_on_actual_state_and_ast_sees_reality() {
        let nodes = vec![
            node("n-alive", "a.jp", "alive", 0.9),
            node("n-degraded", "b.jp", "degraded", 0.9),
            node("n-dead", "c.jp", "dead", 0.9),
            node("n-unknown", "d.jp", "unknown", 0.9),
        ];

        // Bar "alive": only alive passes; degraded/dead/unknown are out.
        let mut spec = profile("");
        spec.health = "alive".into();
        let out = render_subscription(
            &nodes,
            &spec,
            None,
            200,
            &GradeThresholds::default(),
            "",
            "yaml",
        )
        .unwrap();
        let RenderedSubscription::Mihomo(y) = out else {
            panic!("yaml")
        };
        assert!(y.contains("a.jp"), "alive must pass: {y}");
        assert!(
            !y.contains("b.jp") && !y.contains("c.jp") && !y.contains("d.jp"),
            "below-bar health must be excluded: {y}"
        );

        // Bar "degraded": alive and degraded pass, dead/unknown do not.
        let mut spec = profile("");
        spec.health = "degraded".into();
        let out = render_subscription(
            &nodes,
            &spec,
            None,
            200,
            &GradeThresholds::default(),
            "",
            "yaml",
        )
        .unwrap();
        let RenderedSubscription::Mihomo(y) = out else {
            panic!("yaml")
        };
        assert!(
            y.contains("a.jp") && y.contains("b.jp"),
            "alive+degraded pass: {y}"
        );
        assert!(
            !y.contains("c.jp") && !y.contains("d.jp"),
            "dead/unknown excluded: {y}"
        );

        // No bar: everything passes (the AST is the only health gate then).
        let out = render_subscription(
            &nodes,
            &profile(""),
            None,
            200,
            &GradeThresholds::default(),
            "",
            "yaml",
        )
        .unwrap();
        let RenderedSubscription::Mihomo(y) = out else {
            panic!("yaml")
        };
        assert!(
            y.contains("c.jp") && y.contains("d.jp"),
            "no bar = no health gate: {y}"
        );

        // AST health condition sees the REAL state even when the bar is disabled.
        let ast = r#"{"operator":"and","conditions":[
            {"field":"health","operator":"eq","value":"dead"}]}"#;
        let out = render_subscription(
            &nodes,
            &profile(ast),
            None,
            200,
            &GradeThresholds::default(),
            "",
            "yaml",
        )
        .unwrap();
        let RenderedSubscription::Mihomo(y) = out else {
            panic!("yaml")
        };
        assert!(y.contains("c.jp"), "AST must match real dead state: {y}");
        assert!(
            !y.contains("a.jp"),
            "AST must not see the profile wish: {y}"
        );
    }

    fn profile(ast: &str) -> ProfileSpec {
        ProfileSpec {
            filter_ast: ast.into(),
            region: None,
            health: String::new(),
            min_score: 0.0,
            limit_count: 200,
            preset: "all".into(),
            naming_rule: None,
        }
    }

    fn node(id: &str, server: &str, health: &str, score: f64) -> PipelineNode {
        PipelineNode {
            id: id.into(),
            region: "JP".into(),
            score,
            health: health.into(),
            clash_proxy: serde_json::json!({"type":"trojan","server":server,"port":443,"password":"pw"}),
            protocol: "trojan".into(),
            server: server.into(),
            port: 443,
            credential: "pw".into(),
            tls_sni: None,
            ws_path: None,
            streaming: std::collections::HashMap::new(),
        }
    }

    /// Acceptance 2: AST conditions hold for every output node.
    #[test]
    fn filter_ast_applies() {
        let nodes = sample_nodes();
        let ast = r#"{"operator":"and","conditions":[
            {"field":"region","operator":"eq","value":"JP"},
            {"field":"score","operator":"gte","value":0.7}]}"#;
        let out = render_subscription(
            &nodes,
            &profile(ast),
            None,
            200,
            &GradeThresholds::default(),
            "",
            "yaml",
        )
        .unwrap();
        let RenderedSubscription::Mihomo(yaml) = out else {
            panic!("yaml")
        };
        assert!(yaml.contains("JP"), "yaml: {yaml}");
        assert!(
            !yaml.contains("US"),
            "filtered node must not appear: {yaml}"
        );
    }

    /// Acceptance 3/4 (pure part): revoked token path is a caller-side guard; here we
    /// verify the pipeline rejects empty results and formats are distinct.
    #[test]
    fn formats_and_empty_guard() {
        let nodes = sample_nodes();
        let out = render_subscription(
            &nodes,
            &profile(""),
            None,
            200,
            &GradeThresholds::default(),
            "",
            "json",
        )
        .unwrap();
        let RenderedSubscription::Singbox(json) = out else {
            panic!("json")
        };
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["type"] == "trojan"));

        let out = render_subscription(
            &nodes,
            &profile(""),
            None,
            200,
            &GradeThresholds::default(),
            "",
            "base64",
        )
        .unwrap();
        let RenderedSubscription::Base64(b64) = out else {
            panic!("b64")
        };
        use base64::Engine;
        assert!(base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .is_ok());

        let empty: Vec<PipelineNode> = vec![];
        assert!(render_subscription(
            &empty,
            &profile(""),
            None,
            200,
            &GradeThresholds::default(),
            "",
            "yaml"
        )
        .is_err());
        assert!(
            render_subscription(
                &nodes,
                &profile(""),
                None,
                200,
                &GradeThresholds::default(),
                "",
                "bogus"
            )
            .is_err(),
            "unknown format must be rejected"
        );
        let _ = DEFAULT_SINGBOX_TEMPLATE;
    }
}
