use std::time::Duration;

use crate::score::Metrics;
use crate::score::Scored;

const PREVIEW_CHARS: usize = 60;
const MEDIAN: usize = 50;
const P95: usize = 95;

pub(crate) fn queries(scored: &[Scored]) -> Vec<String> {
    let mut out = Vec::new();
    for query in scored {
        out.push(format!("[{}] {:?} ({})", query.id, query.query, query.mode));
        let top = query
            .top
            .as_deref()
            .map_or_else(|| "NONE".to_owned(), preview);
        out.push(format!("  {}  top1={top:?}", millis(query.latency)));
        if let Some(reason) = &query.degraded {
            out.push(format!("  degraded: {reason}"));
        }
        if query.relevant_total > 0 {
            let rank = query.first_relevant.map_or_else(
                || "none in the results".to_owned(),
                |rank| format!("first at rank {rank}"),
            );
            out.push(format!(
                "  relevant: {}/{} found, {rank}",
                query.relevant_found, query.relevant_total
            ));
        }
        if let Some(passed) = query.needle {
            let mark = if passed { "pass" } else { "FAIL" };
            out.push(format!("  must_contain: {mark}"));
        }
    }
    out
}

pub(crate) fn summary(metrics: &Metrics) -> Vec<String> {
    let saved = metrics.baseline();
    let mut out = vec![
        String::new(),
        format!("SUMMARY ({} queries)", saved.query_count),
        format!(
            "recall@{}={:.3}  mrr={:.3}",
            saved.limit, saved.recall_at_k, saved.mrr
        ),
        format!(
            "must_contain: {} pass / {} fail ({:.3})",
            saved.must_contain_passed,
            saved.must_contain_total - saved.must_contain_passed,
            saved.must_contain_pass_rate,
        ),
    ];
    if let (Some(median), Some(p95)) = (metrics.percentile(MEDIAN), metrics.percentile(P95)) {
        out.push(format!(
            "latency p50={}  p95={}",
            millis(median),
            millis(p95)
        ));
    }
    out
}

pub(crate) fn regressions(problems: &[String]) -> Vec<String> {
    let mut out = vec![String::new(), "--- Regression check ---".to_owned()];
    if problems.is_empty() {
        out.push("  all metrics within tolerance".to_owned());
    }
    out.extend(problems.iter().map(|problem| format!("  FAIL {problem}")));
    out
}

fn preview(content: &str) -> String {
    content
        .chars()
        .take(PREVIEW_CHARS)
        .map(|ch| if ch == '\n' { ' ' } else { ch })
        .collect()
}

fn millis(latency: Duration) -> String {
    format!("{:.1}ms", latency.as_secs_f64() * 1000.0)
}
