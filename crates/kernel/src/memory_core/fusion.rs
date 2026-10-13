use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use wire::gateway::memory::MemoryHitSource;

pub const HYBRID_RRF_K: f64 = 20.0;
pub const SUMMARY_FUSION_BONUS: f64 = 1.3;
pub const GATE_FRACTION: f64 = 0.5;
pub const HYBRID_POOL_MIN: i64 = 50;
pub const HYBRID_POOL_MAX: i64 = 100;
pub const HYBRID_MIN_CONTENT_LEN: i64 = 40;

pub trait FusionItem {
    fn fusion_id(&self) -> &str;
    fn fusion_source(&self) -> MemoryHitSource;
    fn fusion_boost(&self) -> f64;
}

#[derive(Debug, Clone, Copy)]
pub struct Fused<'a, T> {
    pub item: &'a T,
    pub rrf: f64,
    pub arm_count: u32,
}

#[must_use]
pub fn hybrid_pool(limit: u32) -> i64 {
    (i64::from(limit) * 3).clamp(HYBRID_POOL_MIN, HYBRID_POOL_MAX)
}

#[expect(
    clippy::expect_used,
    reason = "the literal-scan pattern is a constant; the search tests match it"
)]
#[must_use]
pub fn literal_scan(text: &str) -> bool {
    static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\w[./:_@]\w|\d{1,3}(?:\.\d{1,3}){2,}|[a-z]\d|\d[a-z]")
            .expect("the literal-scan pattern is a constant")
    });
    LITERAL.is_match(text)
}

#[expect(
    clippy::expect_used,
    reason = "the trigram-fallback pattern is a constant; the search tests match it"
)]
#[must_use]
pub fn should_trigram_fallback(text: &str) -> bool {
    static FALLBACK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\S[./:-]\S").expect("the trigram-fallback pattern is a constant")
    });
    literal_scan(text) || FALLBACK.is_match(text)
}

#[must_use]
pub fn reciprocal_rank_fusion<'a, T: FusionItem>(arms: &[&'a [T]], k: f64) -> Vec<Fused<'a, T>> {
    let mut index: BTreeMap<&'a str, usize> = BTreeMap::new();
    let mut fused: Vec<Fused<'a, T>> = Vec::new();
    for arm in arms {
        for (rank, item) in (0u32..).zip(arm.iter()) {
            let contribution = 1.0 / (k + f64::from(rank) + 1.0);
            if let Some(slot) = index
                .get(item.fusion_id())
                .and_then(|&at| fused.get_mut(at))
            {
                slot.rrf += contribution;
                slot.arm_count += 1;
            } else {
                index.insert(item.fusion_id(), fused.len());
                fused.push(Fused {
                    item,
                    rrf: contribution,
                    arm_count: 1,
                });
            }
        }
    }
    fused
}

#[must_use]
pub fn fusion_score<T: FusionItem + ?Sized>(rrf: f64, item: &T) -> f64 {
    let bonus = if item.fusion_source() == MemoryHitSource::Summary {
        SUMMARY_FUSION_BONUS
    } else {
        1.0
    };
    rrf * (1.0 + item.fusion_boost()) * bonus
}

#[must_use]
pub fn passes_fusion_gate(arm_count: u32, score: f64, top: f64) -> bool {
    arm_count >= 2 || score >= top * GATE_FRACTION
}
