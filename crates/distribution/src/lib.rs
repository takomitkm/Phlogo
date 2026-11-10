//! Distribution selector (P5-02): faithful port of Phlogo
//! `internal/distribution/selector.go` — region quotas, node-count limits, and the
//! Efraimidis-Spirakis A-Res weighted sampling. Deterministic branches are assert-equal
//! in tests; the random branches are verified by seeded statistical properties.
use std::collections::{BTreeMap, HashSet};

use rand::Rng;

pub const COMMON_REGIONS: [&str; 5] = ["HK", "TW", "SG", "US", "JP"];
pub const MAX_DISTRIBUTE_DEFAULT: usize = 200;

/// Full naming order from `internal/naming/naming.go` `regionOrder`.
pub const REGION_ORDER: [&str; 45] = [
    "HK", "TW", "JP", "SG", "US", "UK", "GB", "KR", "DE", "FR", "AU", "CA", "NL", "RU", "IN", "BR",
    "TR", "TH", "MY", "PH", "ID", "VN", "AR", "MX", "IT", "ES", "PL", "SE", "NO", "FI", "CH", "AT",
    "BE", "DK", "PT", "CZ", "HU", "RO", "ZA", "AE", "SA", "IL", "EG", "NG", "UA",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    /// Raw region code (may be empty).
    pub region: String,
    /// `node_scores.score` in 0..1.
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectedNode {
    pub candidate: Candidate,
    /// 1-based, within its region.
    pub seq_no: usize,
    /// e.g. `hk01`.
    pub seq_id: String,
}

pub fn normalise_region(r: &str) -> String {
    let t = r.trim().to_uppercase();
    if t.is_empty() {
        "UN".to_string()
    } else {
        t
    }
}

pub fn seq_id(region: &str, seq: usize) -> String {
    format!("{}{:02}", region.to_lowercase(), seq)
}

fn is_common(region: &str) -> bool {
    COMMON_REGIONS.contains(&region)
}

/// Naming-order index; unknown regions sort after known ones alphabetically.
fn region_rank(r: &str) -> (usize, String) {
    match REGION_ORDER.iter().position(|x| *x == r) {
        Some(i) => (i, String::new()),
        None => (REGION_ORDER.len(), r.to_string()),
    }
}

pub fn sort_regions_by_naming_order(regions: &mut [String]) {
    regions.sort_by_key(|r| region_rank(r));
}

pub fn filter_out_common(entries: &[Candidate]) -> Vec<Candidate> {
    entries
        .iter()
        .filter(|e| !is_common(&normalise_region(&e.region)))
        .cloned()
        .collect()
}

pub fn top_n_by_score(pool: &[Candidate], n: usize) -> Vec<Candidate> {
    let mut sorted: Vec<Candidate> = pool.to_vec();
    sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    sorted.into_iter().take(n).collect()
}

pub fn random_sample(pool: &[Candidate], k: usize, rng: &mut impl Rng) -> Vec<Candidate> {
    if k == 0 {
        return vec![];
    }
    if pool.len() <= k {
        return pool.to_vec();
    }
    let mut idxs: Vec<usize> = (0..pool.len()).collect();
    // Partial Fisher-Yates: pick k distinct indices.
    for i in 0..k {
        let j = rng.gen_range(i..pool.len());
        idxs.swap(i, j);
    }
    idxs[..k]
        .iter()
        .map(|i| pool[*i].clone())
        .collect()
}

/// Efraimidis-Spirakis A-Res weighted sampling with a min-heap of size k.
pub fn weighted_sample(pool: &[Candidate], k: usize, rng: &mut impl Rng) -> Vec<Candidate> {
    if k == 0 {
        return vec![];
    }
    if pool.len() <= k {
        return pool.to_vec();
    }
    /// f64 isn't Ord; wrap with a total order on (key, index) for the min-heap.
    #[derive(PartialEq)]
    struct Entry(f64, usize);
    impl Eq for Entry {}
    impl PartialOrd for Entry {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Entry {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            // Reverse of key ordering = min-heap on key; ties break by index (max-heap
            // semantics of BinaryHeap push later wins), matching Go's slice-based heap.
            self.0
                .partial_cmp(&other.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(other.1.cmp(&self.1))
        }
    }

    // Min-heap on key: keep the k largest keys.
    let mut heap: std::collections::BinaryHeap<Entry> = std::collections::BinaryHeap::with_capacity(k);
    for (i, e) in pool.iter().enumerate() {
        let w = e.score.max(0.01);
        let mut u: f64 = rng.gen();
        if u == 0.0 {
            u = 1e-10;
        }
        let key = u.powf(1.0 / w);
        if heap.len() < k {
            heap.push(Entry(key, i));
        } else if key > heap.peek().unwrap().0 {
            heap.pop();
            heap.push(Entry(key, i));
        }
    }
    let mut idxs: Vec<usize> = heap.into_iter().map(|e| e.1).collect();
    idxs.sort_unstable();
    idxs.into_iter().map(|i| pool[i].clone()).collect()
}

fn group_by_region(entries: &[Candidate]) -> BTreeMap<String, Vec<Candidate>> {
    let mut map: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for e in entries {
        map.entry(normalise_region(&e.region))
            .or_default()
            .push(e.clone());
    }
    map
}

/// Quotas: `avg = target / n`; the last region absorbs the remainder.
pub fn compute_quotas(region_sizes: &[usize], target: usize) -> Vec<usize> {
    let n = region_sizes.len();
    if n == 0 {
        return vec![];
    }
    let avg = target / n;
    let mut quotas = vec![avg; n];
    let assigned = avg * (n - 1);
    quotas[n - 1] = target - assigned;
    quotas
}

/// Three-branch pool selection: <=quota take all; <=1.5x top-N; >1.5x weighted.
fn select_from_pool(pool: &[Candidate], quota: usize, rng: &mut impl Rng) -> (Vec<Candidate>, usize) {
    let n = pool.len();
    if n == 0 {
        return (vec![], quota);
    }
    if n <= quota {
        (pool.to_vec(), quota - n)
    } else if (n as f64) <= (quota as f64) * 1.5 {
        (top_n_by_score(pool, quota), 0)
    } else {
        (weighted_sample(pool, quota, rng), 0)
    }
}

/// Common-preset pool pick: full if small, uniform random when over quota.
fn select_from_pool_random(pool: &[Candidate], quota: usize, rng: &mut impl Rng) -> (Vec<Candidate>, usize) {
    let n = pool.len();
    if n == 0 {
        return (vec![], quota);
    }
    if n <= quota {
        (pool.to_vec(), quota - n)
    } else {
        (random_sample(pool, quota, rng), 0)
    }
}

/// Non-UN proportional + UN top-N fill (Go `collectAll`).
pub fn collect_all(entries: &[Candidate], max: usize, rng: &mut impl Rng) -> Vec<Candidate> {
    let (non_un, un): (Vec<Candidate>, Vec<Candidate>) = entries
        .iter()
        .cloned()
        .partition(|e| normalise_region(&e.region) != "UN");
    let mut sel = collect_proportional(&non_un, max, rng);
    let rem = max.saturating_sub(sel.len());
    if rem > 0 {
        sel.extend(top_n_by_score(&un, rem));
    }
    sel
}

/// Proportional quotas per region; deficits refilled by weighted sampling
/// over the unselected remainder (Go `collectProportional`).
fn collect_proportional(entries: &[Candidate], max: usize, rng: &mut impl Rng) -> Vec<Candidate> {
    if entries.is_empty() {
        return vec![];
    }
    let region_map = group_by_region(entries);
    let mut regions: Vec<String> = region_map.keys().cloned().collect();
    sort_regions_by_naming_order(&mut regions);
    let target = max.min(entries.len());
    let sizes: Vec<usize> = regions.iter().map(|r| region_map[r].len()).collect();
    let quotas = compute_quotas(&sizes, target);

    let mut selected: Vec<Candidate> = Vec::new();
    let mut remaining: Vec<Candidate> = Vec::new();
    let mut total_deficit = 0usize;
    for (i, r) in regions.iter().enumerate() {
        let pool = &region_map[r];
        let (got, deficit) = select_from_pool(pool, quotas[i], rng);
        total_deficit += deficit;
        let got_set: HashSet<&str> = got.iter().map(|c| c.id.as_str()).collect();
        for c in pool {
            if !got_set.contains(c.id.as_str()) {
                remaining.push(c.clone());
            }
        }
        selected.extend(got);
    }
    if total_deficit > 0 {
        selected.extend(weighted_sample(&remaining, total_deficit, rng));
    }
    selected
}

/// Common preset (Go `collectCommon`): common regions first with average quotas,
/// uniform-random refill within common, then other-pool and UN by score.
fn collect_common(entries: &[Candidate], max: usize, rng: &mut impl Rng) -> Vec<Candidate> {
    let mut common_pool = Vec::new();
    let mut other_pool = Vec::new();
    let mut un_pool = Vec::new();
    for e in entries {
        match normalise_region(&e.region).as_str() {
            "UN" => un_pool.push(e.clone()),
            r if is_common(r) => common_pool.push(e.clone()),
            _ => other_pool.push(e.clone()),
        }
    }
    let target = max.min(common_pool.len());
    if target == 0 {
        return collect_all(entries, max, rng);
    }

    let region_map = group_by_region(&common_pool);
    let mut regions: Vec<String> = region_map.keys().cloned().collect();
    sort_regions_by_naming_order(&mut regions);
    let sizes: Vec<usize> = regions.iter().map(|r| region_map[r].len()).collect();
    let quotas = compute_quotas(&sizes, target);

    let mut selected: Vec<Candidate> = Vec::new();
    let mut common_remaining: Vec<Candidate> = Vec::new();
    let mut total_deficit = 0usize;
    for (i, r) in regions.iter().enumerate() {
        let pool = &region_map[r];
        let (got, deficit) = select_from_pool_random(pool, quotas[i], rng);
        total_deficit += deficit;
        let got_set: HashSet<&str> = got.iter().map(|c| c.id.as_str()).collect();
        for c in pool {
            if !got_set.contains(c.id.as_str()) {
                common_remaining.push(c.clone());
            }
        }
        selected.extend(got);
    }
    if total_deficit > 0 {
        let need = target - selected.len();
        selected.extend(random_sample(&common_remaining, need, rng));
    }

    let mut rem = max.saturating_sub(selected.len());
    if rem > 0 {
        let take = top_n_by_score(&other_pool, rem);
        rem -= take.len();
        selected.extend(take);
    }
    if rem > 0 {
        selected.extend(top_n_by_score(&un_pool, rem));
    }
    selected
}

/// Group by region in naming order, sort within region by score desc (stable), number seqs.
pub fn assemble_output(nodes: &[Candidate]) -> Vec<SelectedNode> {
    let mut regions: Vec<(String, Vec<Candidate>)> = Vec::new();
    for n in nodes {
        let r = normalise_region(&n.region);
        match regions.iter_mut().find(|(name, _)| *name == r) {
            Some((_, list)) => list.push(n.clone()),
            None => regions.push((r, vec![n.clone()])),
        }
    }
    let mut names: Vec<String> = regions.iter().map(|(r, _)| r.clone()).collect();
    sort_regions_by_naming_order(&mut names);
    let mut out = Vec::new();
    for name in names {
        let (r, mut list) = regions.remove(
            regions
                .iter()
                .position(|(rn, _)| *rn == name)
                .expect("region present"),
        );
        list.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        for (i, c) in list.into_iter().enumerate() {
            let seq_no = i + 1;
            out.push(SelectedNode {
                seq_id: seq_id(&r, seq_no),
                seq_no,
                candidate: c,
            });
        }
    }
    out
}

/// Entry point (Go `SelectWithPreset`). `max == 0` or empty input -> empty output.
pub fn select_with_preset(
    entries: &[Candidate],
    max_distribute: usize,
    preset: &str,
    rng: &mut impl Rng,
) -> Vec<SelectedNode> {
    if entries.is_empty() || max_distribute == 0 {
        return vec![];
    }
    let raw = match preset {
        "other" => collect_all(&filter_out_common(entries), max_distribute, rng),
        "all" => collect_all(entries, max_distribute, rng),
        _ => collect_common(entries, max_distribute, rng),
    };
    let _ = MAX_DISTRIBUTE_DEFAULT;
    assemble_output(&raw)
}

pub mod naming;
pub mod output;
pub mod subscription;
pub mod token;

#[cfg(test)]
mod p5_tests;
