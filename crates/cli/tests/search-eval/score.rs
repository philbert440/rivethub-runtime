use std::time::Duration;

use kernel::memory::SearchMode;
use kernel::memory::SearchOutcome;

use crate::Failure;
use crate::set::Baseline;
use crate::set::EvalQuery;

const NEEDLE_DEPTH: usize = 3;
const PERCENT: usize = 100;
const RATE_SCALE: f64 = 1000.0;
const RATE_EPSILON: f64 = 1e-9;

#[derive(Debug)]
pub(crate) struct Scored {
    pub(crate) id: String,
    pub(crate) query: String,
    pub(crate) mode: SearchMode,
    pub(crate) top: Option<String>,
    pub(crate) relevant_total: u32,
    pub(crate) relevant_found: u32,
    pub(crate) first_relevant: Option<u32>,
    pub(crate) needle: Option<bool>,
    pub(crate) latency: Duration,
    pub(crate) degraded: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Metrics {
    pub(crate) limit: u32,
    pub(crate) query_count: u32,
    pub(crate) recall_at_k: f64,
    pub(crate) mrr: f64,
    pub(crate) must_contain_total: u32,
    pub(crate) must_contain_passed: u32,
    pub(crate) latencies: Vec<Duration>,
}

impl Scored {
    pub(crate) fn new(
        query: &EvalQuery,
        outcome: &SearchOutcome,
        latency: Duration,
    ) -> Result<Self, Failure> {
        let ids: Vec<&str> = outcome.results.iter().map(|hit| hit.id.as_str()).collect();
        let relevant_found = query
            .relevant
            .iter()
            .filter(|id| ids.contains(&id.as_str()))
            .count();
        let first_relevant = ids
            .iter()
            .position(|id| query.relevant.iter().any(|wanted| wanted == id))
            .map(|index| u32::try_from(index + 1))
            .transpose()?;
        let needle = (!query.must_contain.is_empty()).then(|| {
            outcome.results.iter().take(NEEDLE_DEPTH).any(|hit| {
                query
                    .must_contain
                    .iter()
                    .any(|needle| hit.content.contains(needle.as_str()))
            })
        });
        Ok(Self {
            id: query.id.clone(),
            query: query.query.clone(),
            mode: query.mode,
            top: outcome.results.first().map(|hit| hit.content.clone()),
            relevant_total: u32::try_from(query.relevant.len())?,
            relevant_found: u32::try_from(relevant_found)?,
            first_relevant,
            needle,
            latency,
            degraded: outcome
                .degraded
                .as_ref()
                .map(|degraded| degraded.reason.clone()),
        })
    }

    pub(crate) fn missed(&self) -> bool {
        self.needle == Some(false) || (self.relevant_total > 0 && self.relevant_found == 0)
    }

    fn recall(&self) -> Option<f64> {
        (self.relevant_total > 0).then(|| ratio(self.relevant_found, self.relevant_total))
    }

    fn reciprocal_rank(&self) -> Option<f64> {
        (self.relevant_total > 0).then(|| match self.first_relevant {
            Some(rank) => ratio(1, rank),
            None => 0.0,
        })
    }
}

impl Metrics {
    pub(crate) fn of(scored: &[Scored], limit: u32) -> Result<Self, Failure> {
        let recalls: Vec<f64> = scored.iter().filter_map(Scored::recall).collect();
        let ranks: Vec<f64> = scored.iter().filter_map(Scored::reciprocal_rank).collect();
        let checked: Vec<bool> = scored.iter().filter_map(|query| query.needle).collect();
        Ok(Self {
            limit,
            query_count: u32::try_from(scored.len())?,
            recall_at_k: mean(&recalls)?,
            mrr: mean(&ranks)?,
            must_contain_total: u32::try_from(checked.len())?,
            must_contain_passed: u32::try_from(checked.iter().filter(|passed| **passed).count())?,
            latencies: scored.iter().map(|query| query.latency).collect(),
        })
    }

    pub(crate) fn baseline(&self) -> Baseline {
        Baseline {
            limit: self.limit,
            query_count: self.query_count,
            recall_at_k: rounded(self.recall_at_k),
            mrr: rounded(self.mrr),
            must_contain_total: self.must_contain_total,
            must_contain_passed: self.must_contain_passed,
            must_contain_pass_rate: rounded(ratio(
                self.must_contain_passed,
                self.must_contain_total,
            )),
        }
    }

    pub(crate) fn regressions(&self, saved: &Baseline, tolerance: f64) -> Vec<String> {
        let current = self.baseline();
        if current.limit != saved.limit || current.query_count != saved.query_count {
            return vec![format!(
                "baseline was saved for {} queries at limit {}; this run has {} queries at limit {} (regenerate it with --save-baseline)",
                saved.query_count, saved.limit, current.query_count, current.limit
            )];
        }
        [
            ("recall@k", current.recall_at_k, saved.recall_at_k),
            ("mrr", current.mrr, saved.mrr),
            (
                "must_contain_pass_rate",
                current.must_contain_pass_rate,
                saved.must_contain_pass_rate,
            ),
        ]
        .into_iter()
        .filter(|(_, now, then)| then - now > tolerance + RATE_EPSILON)
        .map(|(name, now, then)| {
            format!(
                "{name}: {now:.3} is {:.3} below baseline {then:.3}",
                then - now
            )
        })
        .collect()
    }

    pub(crate) fn percentile(&self, percent: usize) -> Option<Duration> {
        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        let last = sorted.len().checked_sub(1)?;
        let index = sorted.len() * percent / PERCENT;
        sorted.get(index.min(last)).copied()
    }
}

fn ratio(part: u32, whole: u32) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    f64::from(part) / f64::from(whole)
}

fn mean(values: &[f64]) -> Result<f64, Failure> {
    if values.is_empty() {
        return Ok(0.0);
    }
    Ok(values.iter().sum::<f64>() / f64::from(u32::try_from(values.len())?))
}

fn rounded(value: f64) -> f64 {
    (value * RATE_SCALE).round() / RATE_SCALE
}
