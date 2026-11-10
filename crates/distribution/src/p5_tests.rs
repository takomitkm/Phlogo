//! P5-02 acceptance tests: deterministic branches exact, random branches seeded/statistical.
use crate::{assemble_output, normalise_region, select_with_preset, top_n_by_score, Candidate};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

fn cand(id: &str, region: &str, score: f64) -> Candidate {
    Candidate { id: id.into(), region: region.into(), score }
}

fn rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}

/// A1: pool <= quota -> take all.
#[test]
fn pool_under_quota_take_all() {
    let entries: Vec<Candidate> = (0..3).map(|i| cand(&format!("n{i}"), "DE", 0.5)).collect();
    let out = select_with_preset(&entries, 10, "all", &mut rng(1));
    assert_eq!(out.len(), 3);
}

/// A2: pool <= 1.5x quota -> top-N by score.
#[test]
fn pool_near_quota_top_n() {
    let entries: Vec<Candidate> = (0..6)
        .map(|i| cand(&format!("n{i}"), "DE", 0.1 * (i as f64 + 1.0)))
        .collect();
    let out = select_with_preset(&entries, 5, "all", &mut rng(2));
    assert_eq!(out.len(), 5);
    // Highest scores win; all from the pool.
    let picked: Vec<&str> = out.iter().map(|s| s.candidate.id.as_str()).collect();
    assert!(picked.contains(&"n5") && picked.contains(&"n2"));
    assert!(!picked.contains(&"n0"), "lowest score must drop in top-N branch");
}

/// A3: pool > 1.5x quota -> weighted sampling triggers, exactly k members.
#[test]
fn pool_over_quota_weighted() {
    let entries: Vec<Candidate> = (0..10)
        .map(|i| cand(&format!("n{i}"), "US", 0.2 + (i as f64) * 0.05))
        .collect();
    let out = select_with_preset(&entries, 5, "all", &mut rng(3));
    assert_eq!(out.len(), 5);
    for s in &out {
        assert!(entries.iter().any(|e| e.id == s.candidate.id));
    }
}

/// A4: common quotas split evenly across the 5 common regions.
#[test]
fn common_even_quota() {
    let mut entries = Vec::new();
    for r in ["HK", "TW", "SG", "US", "JP"] {
        for i in 0..10 {
            entries.push(cand(&format!("{r}{i}"), r, 0.3));
        }
    }
    let out = select_with_preset(&entries, 20, "common", &mut rng(4));
    assert_eq!(out.len(), 20);
    let mut per: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in &out {
        *per.entry(normalise_region(&s.candidate.region)).or_default() += 1;
    }
    for r in ["HK", "TW", "SG", "US", "JP"] {
        assert_eq!(per.get(r), Some(&4), "region {r}");
    }
}

/// A5: no common-region nodes -> common falls back to collect_all behavior
/// (same seed => identical membership, since the fallback path is the same code).
#[test]
fn common_fallback() {
    let mut entries = Vec::new();
    for i in 0..8 {
        entries.push(cand(&format!("DE{i}"), "DE", 0.2 + i as f64 * 0.05));
    }
    let out = select_with_preset(&entries, 5, "common", &mut rng(5));
    let all = select_with_preset(&entries, 5, "all", &mut rng(5));
    assert_eq!(out.len(), 5);
    let a: std::collections::HashSet<&str> = out.iter().map(|s| s.candidate.id.as_str()).collect();
    let b: std::collections::HashSet<&str> = all.iter().map(|s| s.candidate.id.as_str()).collect();
    assert_eq!(a, b, "fallback must equal all-selection (same seed)");
}

/// A6: other preset excludes the five common regions.
#[test]
fn other_excludes_common() {
    let mut entries = Vec::new();
    for r in ["HK", "TW", "DE", "FR"] {
        for i in 0..4 {
            entries.push(cand(&format!("{r}{i}"), r, 0.4));
        }
    }
    let out = select_with_preset(&entries, 100, "other", &mut rng(7));
    assert_eq!(out.len(), 8, "only DE/FR survive");
    for s in &out {
        let r = normalise_region(&s.candidate.region);
        assert!(!["HK", "TW", "SG", "US", "JP"].contains(&r.as_str()));
    }
}

/// A7: UN fills the remainder by top score.
#[test]
fn un_fills_remainder() {
    let mut entries = Vec::new();
    for i in 0..2 {
        entries.push(cand(&format!("DE{i}"), "DE", 0.5));
    }
    for i in 0..5 {
        entries.push(cand(&format!("UN{i}"), "", 0.1 * (i as f64 + 1.0)));
    }
    let out = select_with_preset(&entries, 6, "all", &mut rng(8));
    assert_eq!(out.len(), 6);
    let un: Vec<&str> = out
        .iter()
        .filter(|s| normalise_region(&s.candidate.region) == "UN")
        .map(|s| s.candidate.id.as_str())
        .collect();
    assert_eq!(un.len(), 4, "UN fills 6-2");
    assert!(un.contains(&"UN4"), "top-scored UN wins");
}

/// A8: output ordering = naming order; within region score desc; seq_id format.
#[test]
fn output_ordering_and_seq() {
    let entries = vec![
        cand("hk1", "hk", 0.5),
        cand("hk2", "HK", 0.9),
        cand("jp1", "JP", 0.7),
        cand("zz1", "ZZ", 0.4),
        cand("de1", "DE", 0.6),
    ];
    let out = assemble_output(&entries);
    let ids: Vec<&str> = out.iter().map(|s| s.candidate.id.as_str()).collect();
    assert_eq!(ids, vec!["hk2", "hk1", "jp1", "de1", "zz1"], "naming order + score desc");
    assert_eq!(out[0].seq_id, "hk01");
    assert_eq!(out[1].seq_id, "hk02");
    assert_eq!(out[2].seq_id, "jp01");
    assert_eq!(out[3].seq_id, "de01");
    assert_eq!(out[4].seq_id, "zz01");
}

/// B9: weighted sample ranks positively with weight; lowest weight not excluded.
#[test]
fn weighted_sample_statistical() {
    let pool: Vec<Candidate> = (0..10)
        .map(|i| cand(&format!("n{i}"), "US", if i == 0 { 0.01 } else { 0.1 + i as f64 * 0.09 }))
        .collect();
    let mut counts = [0usize; 10];
    for seed in 0..1000u64 {
        let mut r = rng(seed);
        let picked = crate::weighted_sample(&pool, 1, &mut r);
        let idx = pool.iter().position(|c| c.id == picked[0].id).unwrap();
        counts[idx] += 1;
    }
    // Highest-score node (n9) must be picked more often than the lowest (n0).
    assert!(counts[9] > counts[0], "high weight must win more: n9={}, n0={}", counts[9], counts[0]);
    assert!(counts[0] > 0, "low weight must not be fully excluded (w>=0.01)");
    let _ = top_n_by_score(&pool, 1); // completeness
}

/// B10: uniform random over quota is ~50% for 10 nodes / quota 5.
#[test]
fn random_sample_uniform() {
    let pool: Vec<Candidate> = (0..10).map(|i| cand(&format!("n{i}"), "US", 0.5)).collect();
    let mut counts = [0usize; 10];
    for seed in 0..1000u64 {
        let mut r = rng(seed + 10_000);
        let picked = crate::random_sample(&pool, 5, &mut r);
        for p in picked {
            let idx = pool.iter().position(|c| c.id == p.id).unwrap();
            counts[idx] += 1;
        }
    }
    for (i, c) in counts.iter().enumerate() {
        assert!((430..=570).contains(c), "node {i} freq {c} outside +-7%");
    }
}

/// C11: preset semantics over a representative candidate set.
#[test]
fn preset_semantics_end_to_end() {
    let mut entries = Vec::new();
    for r in ["HK", "TW", "SG", "US", "JP"] {
        for i in 0..6 {
            entries.push(cand(&format!("{r}{i}"), r, 0.3 + (i as f64) * 0.05));
        }
    }
    for i in 0..8 {
        entries.push(cand(&format!("DE{i}"), "DE", 0.2 + i as f64 * 0.05));
    }
    for i in 0..4 {
        entries.push(cand(&format!("UN{i}"), "", 0.1));
    }

    let common = select_with_preset(&entries, 20, "common", &mut rng(11));
    assert!(common.len() <= 20);
    let commons: usize = common
        .iter()
        .filter(|s| ["HK", "TW", "SG", "US", "JP"].contains(&normalise_region(&s.candidate.region).as_str()))
        .count();
    assert!(commons >= 15, "common must prioritize the 5 common regions: {commons}");

    let all = select_with_preset(&entries, 20, "all", &mut rng(12));
    assert!(all.len() <= 20 && !all.is_empty());
    let uns: usize = all
        .iter()
        .filter(|s| normalise_region(&s.candidate.region) == "UN")
        .count();
    assert!(uns < 20, "UN only fills the remainder");

    let other = select_with_preset(&entries, 20, "other", &mut rng(13));
    assert!(other.len() <= 20 && !other.is_empty());
    for s in &other {
        assert!(!["HK", "TW", "SG", "US", "JP"].contains(&normalise_region(&s.candidate.region).as_str()));
    }
    // UN may fill the remainder (docs/distribution.md: UN joins when candidates are short),
    // but the five common regions must never appear.
}

/// C12: selection never exceeds max_distribute.
#[test]
fn never_exceeds_max() {
    let mut entries = Vec::new();
    for r in ["HK", "TW", "SG", "US", "JP", "DE", "FR", "GB"] {
        for i in 0..50 {
            entries.push(cand(&format!("{r}{i}"), r, 0.1 + (i as f64) * 0.01));
        }
    }
    for preset in ["common", "all", "other"] {
        let out = select_with_preset(&entries, 200, preset, &mut rng(21));
        assert!(out.len() <= 200, "preset {preset}");
        let out = select_with_preset(&entries, 37, preset, &mut rng(22));
        assert!(out.len() <= 37, "preset {preset}");
        let out = select_with_preset(&entries, 0, preset, &mut rng(23));
        assert!(out.is_empty(), "max=0 must yield empty");
    }
}
