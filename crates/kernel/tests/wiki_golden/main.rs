#![cfg(test)]

mod page;
mod skips;
mod text;

use std::path::PathBuf;

use testkit::wiki_golden::Corpus;

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) fn corpus() -> Corpus {
    Corpus::new(
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/wiki")),
        skips::SKIPS,
    )
}

#[test]
fn every_corpus_file_is_replayed_or_unported() -> TestResult {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(corpus().dir())? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 name")?;
        if let Some(function) = name.strip_suffix(".jsonl") {
            files.push(function.to_owned());
        }
    }
    files.sort();
    let mut covered: Vec<String> = text::REPLAYED
        .iter()
        .chain(page::REPLAYED)
        .chain(skips::UNPORTED.iter().map(|(function, _)| function))
        .map(|function| (*function).to_owned())
        .collect();
    covered.sort();
    assert_eq!(files, covered);
    for skip in skips::SKIPS {
        assert!(
            !skip.reason.is_empty() && !skip.lines.is_empty(),
            "{skip:?}"
        );
        assert!(
            text::REPLAYED.contains(&skip.function) || page::REPLAYED.contains(&skip.function),
            "{skip:?}"
        );
    }
    Ok(())
}
