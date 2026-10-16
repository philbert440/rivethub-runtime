use std::convert::Infallible;

use wire::wiki::{
    ArticlePatch, ArticleSection, ArticleSections, CappedSummary, PageCitation,
    apply_article_patches, cap_summary, demote_h2_headings, extract_wiki_links,
    join_article_sections, merge_summary_text, normalize_slug, parse_citations, parse_see_also,
    split_article_sections,
};

use crate::harness::{TestResult, never_thrown, replay};

pub(crate) const REPLAYED: &[&str] = &[
    "normalize_slug",
    "extract_wiki_links",
    "demote_h2_headings",
    "split_article_sections",
    "join_article_sections",
    "apply_article_patches",
    "parse_see_also",
    "parse_citations",
    "merge_summary_text",
    "cap_summary",
];

#[derive(serde::Deserialize)]
struct CapArgs(String, #[serde(default)] Option<usize>);

pub(crate) fn pure<A, O>(run: impl Fn(A) -> O) -> impl Fn(A) -> Result<O, Infallible> {
    move |input| Ok(run(input))
}

pub(crate) fn report(function: &str, result: Result<crate::harness::Tally, String>) -> TestResult {
    let tally = result.inspect_err(|failures| println!("{failures}"))?;
    println!(
        "{function}: {} passed, {} skipped",
        tally.passed, tally.skipped
    );
    Ok(())
}

#[test]
fn normalize_slug_replays() -> TestResult {
    report(
        "normalize_slug",
        replay(
            "normalize_slug",
            pure(|text: String| normalize_slug(&text)),
            never_thrown,
        ),
    )
}

#[test]
fn extract_wiki_links_replays() -> TestResult {
    report(
        "extract_wiki_links",
        replay(
            "extract_wiki_links",
            pure(|text: String| extract_wiki_links(&text)),
            never_thrown,
        ),
    )
}

#[test]
fn demote_h2_headings_replays() -> TestResult {
    report(
        "demote_h2_headings",
        replay(
            "demote_h2_headings",
            pure(|text: String| demote_h2_headings(&text)),
            never_thrown,
        ),
    )
}

#[test]
fn split_article_sections_replays() -> TestResult {
    report(
        "split_article_sections",
        replay(
            "split_article_sections",
            pure(|text: String| -> ArticleSections { split_article_sections(&text) }),
            never_thrown,
        ),
    )
}

#[test]
fn join_article_sections_replays() -> TestResult {
    report(
        "join_article_sections",
        replay(
            "join_article_sections",
            pure(|(lead, sections): (String, Vec<ArticleSection>)| {
                join_article_sections(&lead, &sections)
            }),
            never_thrown,
        ),
    )
}

#[test]
fn apply_article_patches_replays() -> TestResult {
    report(
        "apply_article_patches",
        replay(
            "apply_article_patches",
            pure(|(article, patches): (String, Vec<ArticlePatch>)| {
                apply_article_patches(&article, &patches)
            }),
            never_thrown,
        ),
    )
}

#[test]
fn parse_see_also_replays() -> TestResult {
    report(
        "parse_see_also",
        replay(
            "parse_see_also",
            pure(|text: String| parse_see_also(&text)),
            never_thrown,
        ),
    )
}

#[test]
fn parse_citations_replays() -> TestResult {
    report(
        "parse_citations",
        replay(
            "parse_citations",
            pure(|text: String| -> Vec<PageCitation> { parse_citations(&text) }),
            never_thrown,
        ),
    )
}

#[test]
fn merge_summary_text_replays() -> TestResult {
    report(
        "merge_summary_text",
        replay(
            "merge_summary_text",
            pure(|(existing, delta): (String, String)| merge_summary_text(&existing, &delta)),
            never_thrown,
        ),
    )
}

#[test]
fn cap_summary_replays() -> TestResult {
    report(
        "cap_summary",
        replay(
            "cap_summary",
            pure(|CapArgs(text, limit)| -> CappedSummary {
                cap_summary(&text, limit.unwrap_or(wire::wiki::SUMMARY_LIMIT))
            }),
            never_thrown,
        ),
    )
}
