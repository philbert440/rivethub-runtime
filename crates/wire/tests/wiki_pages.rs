#![cfg(test)]

use wire::wiki::{
    build_wiki_search_text, extract_wiki_links, parse_wiki_page, serialize_wiki_page,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const REPLAYED: &[&str] = &["kiwi-build-cache.md", "lighthouse-dashboard.md"];
const SKIPPED: &[(&str, &str)] = &[];

#[test]
fn every_vendored_page_fixture_is_accounted_for_and_round_trips() -> TestResult {
    let directory = testkit::recording("mcp-memory/shared/wiki/topics");
    let mut count = 0;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("non-UTF8 fixture name")?;
        let replayed = REPLAYED
            .iter()
            .filter(|candidate| **candidate == name)
            .count();
        let skipped = SKIPPED
            .iter()
            .filter(|(candidate, reason)| *candidate == name && !reason.is_empty())
            .count();
        assert_eq!(replayed + skipped, 1, "{name}");
        if replayed == 1 {
            let page = parse_wiki_page(&std::fs::read_to_string(entry.path())?)?;
            let encoded = serialize_wiki_page(&page)?;
            assert_eq!(parse_wiki_page(&encoded)?, page, "{name}");
            assert_eq!(page.history.len(), 1);
            assert_ne!(page.citations, Vec::new());
            assert!(
                build_wiki_search_text(&page)
                    .contains(&page.history.first().ok_or("history")?.body)
            );
        }
        count += 1;
    }
    assert_eq!(count, REPLAYED.len() + SKIPPED.len());
    Ok(())
}

#[test]
fn frontmatter_extras_preamble_and_markdown_fences_survive() -> TestResult {
    let markdown = "---\ntitle: Notes\nslug: notes\ncustom:\n  owner: team\n  count: 3\n---\n\nA human preamble.\n\n## Summary\n\nUseful summary.\n\n## Article\n\n```markdown\n## History\n```\n\n## Troubleshooting\n\nKeep this section too.\n";
    let page = parse_wiki_page(markdown)?;
    assert_eq!(page.preamble, "A human preamble.");
    assert!(
        page.article
            .as_deref()
            .ok_or("article")?
            .contains("## History")
    );
    let expected: wire::Extra = serde_json::from_str(r#"{"custom":{"owner":"team","count":3}}"#)?;
    assert_eq!(page.frontmatter.extra, expected);
    assert_eq!(parse_wiki_page(&serialize_wiki_page(&page)?)?, page);
    Ok(())
}

#[test]
fn malformed_pages_are_errors_and_links_keep_unicode() {
    for text in [
        "no frontmatter",
        "---\ntitle: x\n",
        "---\ntitle: []\nslug: a\n---\n",
        "---\ntitle: x\nslug: ../x\n---\n",
        "---\ntitle: x\nslug: x\n---\n## History\nnot a dated entry",
    ] {
        assert!(parse_wiki_page(text).is_err(), "{text}");
    }
    assert_eq!(
        extract_wiki_links("[[café|Café]] [[café#section]] [[🦀]] [[broken"),
        ["café", "🦀"]
    );
}
