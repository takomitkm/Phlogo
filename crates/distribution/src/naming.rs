//! Node naming (P5-03 §5): a fixed, safe template renderer over whitelisted fields —
//! the expr-template migration target (`node_name_template` consumers call this).
use super::{normalise_region, SelectedNode};

/// Grade thresholds (config `grade_{a,b,c}_min`, defaults per CONVENTIONS §2.6).
#[derive(Debug, Clone, Copy)]
pub struct GradeThresholds {
    pub a_min: f64,
    pub b_min: f64,
    pub c_min: f64,
}

impl Default for GradeThresholds {
    fn default() -> Self {
        Self { a_min: 0.75, b_min: 0.50, c_min: 0.25 }
    }
}

pub fn grade_from_score(score: f64, t: &GradeThresholds) -> &'static str {
    if score >= t.a_min {
        "A"
    } else if score >= t.b_min {
        "B"
    } else if score >= t.c_min {
        "C"
    } else {
        "D"
    }
}

/// Emoji flags for the naming order regions (subset used by default templates).
pub fn flag_for(region: &str) -> &'static str {
    match region {
        "HK" => "🇭🇰",
        "TW" => "🇹🇼",
        "JP" => "🇯🇵",
        "SG" => "🇸🇬",
        "US" => "🇺🇸",
        "UK" | "GB" => "🇬🇧",
        "KR" => "🇰🇷",
        "DE" => "🇩🇪",
        "FR" => "🇫🇷",
        "AU" => "🇦🇺",
        "CA" => "🇨🇦",
        "NL" => "🇳🇱",
        "RU" => "🇷🇺",
        "IN" => "🇮🇳",
        "BR" => "🇧🇷",
        "UN" => "🏳️",
        _ => "🌐",
    }
}

/// Fields a template may reference (whitelist; no arbitrary code evaluation).
#[derive(Debug, Clone)]
pub struct NameFields {
    pub region: String,
    pub region_flag: &'static str,
    pub grade: &'static str,
    pub score: f64,
    pub seq: String,
    pub server: String,
    pub protocol: String,
    pub sponsor: String,
    pub unlock_flags: String,
}

pub fn fields_for(sel: &SelectedNode, thresholds: &GradeThresholds, unlock_flags: &str) -> NameFields {
    let region = normalise_region(&sel.candidate.region);
    NameFields {
        region_flag: flag_for(&region),
        region,
        grade: grade_from_score(sel.candidate.score, thresholds),
        score: sel.candidate.score,
        seq: format!("{:02}", sel.seq_no),
        server: String::new(),
        protocol: String::new(),
        sponsor: String::new(),
        unlock_flags: unlock_flags.to_string(),
    }
}

/// Render the default template shape `(flags != "" ? "[flags] " : "") + grade + " - " +
/// flag + " " + region + " " + seq + (sponsor != "" ? " " + sponsor : "")`.
/// Templates are `{{field}}` placeholders over the whitelist only.
pub fn render(template: &str, f: &NameFields) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let close = template[i + 2..].find("}}").map(|p| i + 2 + p);
            if let Some(end) = close {
                let key = template[i + 2..end].trim();
                out.push_str(&field_value(f, key));
                i = end + 2;
                continue;
            }
        }
        out.push(template[i..].chars().next().expect("non-empty char slice"));
        i += template[i..]
            .chars()
            .next()
            .expect("non-empty char slice")
            .len_utf8();
    }
    out
}

fn field_value(f: &NameFields, key: &str) -> String {
    match key {
        "region" => f.region.clone(),
        "region_flag" => f.region_flag.to_string(),
        "grade" => f.grade.to_string(),
        "score" => format!("{:.2}", f.score),
        "seq" => f.seq.clone(),
        "server" => f.server.clone(),
        "protocol" => f.protocol.clone(),
        "sponsor" => f.sponsor.clone(),
        "unlock_flags" => f.unlock_flags.clone(),
        _ => String::new(),
    }
}

/// Default template equivalent to the seeded `node_name_template` (AST-free form).
pub const DEFAULT_TEMPLATE: &str =
    "{{unlock_flags}}{{region_flag}} {{region}} {{seq}}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Candidate;

    fn sel(region: &str, score: f64, seq: usize) -> SelectedNode {
        SelectedNode {
            candidate: Candidate { id: format!("{region}{seq}"), region: region.into(), score },
            seq_no: seq,
            seq_id: format!("{}{:02}", region.to_lowercase(), seq),
        }
    }

    #[test]
    fn grades() {
        let t = GradeThresholds::default();
        assert_eq!(grade_from_score(0.9, &t), "A");
        assert_eq!(grade_from_score(0.6, &t), "B");
        assert_eq!(grade_from_score(0.3, &t), "C");
        assert_eq!(grade_from_score(0.1, &t), "D");
    }

    #[test]
    fn default_template_render() {
        let s = sel("HK", 0.9, 3);
        let f = fields_for(&s, &GradeThresholds::default(), "");
        assert_eq!(render(DEFAULT_TEMPLATE, &f), "🇭🇰 HK 03");
        let f2 = fields_for(&s, &GradeThresholds::default(), "[NF] ");
        assert_eq!(render(DEFAULT_TEMPLATE, &f2), "[NF] 🇭🇰 HK 03");
    }

    #[test]
    fn fallback_name() {
        // Template failure fallback = "<region> <seq>" (naming.go).
        let s = sel("JP", 0.5, 12);
        assert_eq!(format!("{} {}", normalise_region(&s.candidate.region), s.seq_id), "JP jp12");
    }
}
