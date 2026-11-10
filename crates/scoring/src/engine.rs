//! `compute_score` port (scoring.go:113-188). Records must be newest-first (`i = 0`).
pub const SCORE_VERSION: &str = "ewma-v1";

const MAX_LATENCY_MS: f64 = 3000.0;
const MAX_JITTER_MS: f64 = 1000.0;
const MAX_SPEED_KBPS: f64 = 163840.0;

#[derive(Debug, Clone, Copy)]
pub struct DeepCheck {
    /// -1 = unreachable (mapped to MAX_LATENCY_MS).
    pub avg_latency_ms: i32,
    /// -1 = unreachable (mapped to MAX_JITTER_MS).
    pub jitter_ms: i32,
    /// Untested arrives as 0 per `buildCheckRecords`.
    pub speed_kbps: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct ScoreParams {
    pub alpha: f64,
    pub w_latency: f64,
    pub w_stability: f64,
    pub w_speed: f64,
    pub latency_curve_k: f64,
    pub jitter_linear_threshold_ms: f64,
}

impl Default for ScoreParams {
    fn default() -> Self {
        Self {
            alpha: 0.7,
            w_latency: 0.7,
            w_stability: 0.3,
            w_speed: 0.55,
            latency_curve_k: 0.3,
            jitter_linear_threshold_ms: 200.0, // authoritative: config seed (Go fallback was 70)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreStatus {
    Ok,
    /// net-new: fewer than `score_min_samples` records; score not written.
    InsufficientData,
}

#[derive(Debug, Clone)]
pub struct ScoreComponents {
    pub weighted_latency_ms: f64,
    pub weighted_jitter_ms: f64,
    pub latency_score: f64,
    pub stability_score: f64,
    pub quality_score: f64,
    pub speed_score: Option<f64>,
    pub sample_count: usize,
}

#[derive(Debug, Clone)]
pub struct ScoreResult {
    pub score: f64,
    pub status: ScoreStatus,
    pub version: String,
    pub components: ScoreComponents,
}

pub fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// Compute the EWMA baseline score + explanation components.
pub fn compute_score(records: &[DeepCheck], p: &ScoreParams) -> ScoreResult {
    if records.is_empty() {
        return ScoreResult {
            score: 0.0,
            status: ScoreStatus::InsufficientData,
            version: SCORE_VERSION.into(),
            components: ScoreComponents {
                weighted_latency_ms: 0.0,
                weighted_jitter_ms: 0.0,
                latency_score: 0.0,
                stability_score: 0.0,
                quality_score: 0.0,
                speed_score: None,
                sample_count: 0,
            },
        };
    }
    let mut alpha = p.alpha;
    if alpha <= 0.0 || alpha > 1.0 {
        alpha = 0.7;
    }

    let (mut sum_w, mut sum_lat, mut sum_jit) = (0.0_f64, 0.0, 0.0);
    for (i, r) in records.iter().enumerate() {
        let mut latency = r.avg_latency_ms as f64;
        let mut jitter = r.jitter_ms as f64;
        if latency == -1.0 {
            latency = MAX_LATENCY_MS;
        }
        if jitter == -1.0 {
            jitter = MAX_JITTER_MS;
        }
        let w = alpha.powi(i as i32);
        sum_w += w;
        sum_lat += w * latency;
        sum_jit += w * jitter;
    }
    let w_lat = sum_lat / sum_w;
    let w_jit = sum_jit / sum_w;

    let latency_x = clamp01(w_lat / MAX_LATENCY_MS);
    let mut k = p.latency_curve_k;
    if k <= 0.0 {
        k = 0.3;
    }
    let latency_score = 1.0 - (1.0 + k * latency_x).ln() / (1.0 + k).ln();

    let jitter_x = clamp01(w_jit / MAX_JITTER_MS);
    let mut t = clamp01(p.jitter_linear_threshold_ms / MAX_JITTER_MS);
    if t <= 0.0 || t >= 1.0 {
        t = 70.0 / MAX_JITTER_MS; // Go loadParams fallback
    }
    let stability_score = if jitter_x <= t {
        1.0 - jitter_x
    } else {
        (1.0 - t) * ((1.0 - jitter_x) / (1.0 - t)).sqrt()
    };

    let w_lat_w = clamp01(p.w_latency);
    let w_stab = clamp01(p.w_stability);
    let w_speed = clamp01(p.w_speed);
    let total_q = w_lat_w + w_stab;
    let (w_lat_adj, w_stab_adj) = if total_q <= 0.0 {
        (0.7, 0.3)
    } else {
        (w_lat_w / total_q, w_stab / total_q)
    };
    let quality = latency_score * w_lat_adj + stability_score * w_stab_adj;

    let mut score = quality;
    let mut speed_score = None;
    let speed = records[0].speed_kbps as f64; // latest record only (Go baseline)
    if speed >= 0.0 {
        let speed_x = clamp01(speed / MAX_SPEED_KBPS);
        let s = speed_x.sqrt();
        speed_score = Some(s);
        score = quality * (1.0 - w_speed) + s * w_speed;
    }
    let score = (score * 1e6).round() / 1e6;

    ScoreResult {
        score,
        status: ScoreStatus::Ok,
        version: SCORE_VERSION.into(),
        components: ScoreComponents {
            weighted_latency_ms: w_lat,
            weighted_jitter_ms: w_jit,
            latency_score,
            stability_score,
            quality_score: quality,
            speed_score,
            sample_count: records.len(),
        },
    }
}
