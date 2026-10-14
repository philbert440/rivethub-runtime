use std::path::Path;

use proc_macro2::Span;
use syn::visit::Visit;

use crate::error::Result;
use crate::git::tracked_files;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub file: String,
    pub line: usize,
    pub text: String,
}

pub fn is_scanned(rel: &str) -> bool {
    rel.starts_with("crates/") && Path::new(rel).extension().is_some_and(|ext| ext == "rs")
}

pub fn in_wire(rel: &str) -> bool {
    rel.starts_with("crates/wire/")
}

pub fn read_sources(root: &Path) -> Result<Vec<(String, syn::File)>> {
    tracked_files(root)?
        .into_iter()
        .filter(|rel| is_scanned(rel))
        .map(|rel| {
            let text = std::fs::read_to_string(root.join(&rel))?;
            let file = syn::parse_file(&text)?;
            Ok((rel, file))
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct Hits {
    pub items: Vec<(&'static str, Hit)>,
    file: String,
}

impl Hits {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_owned(),
            ..Self::default()
        }
    }

    pub fn push(&mut self, check: &'static str, span: Span, text: impl Into<String>) {
        self.items.push((
            check,
            Hit {
                file: self.file.clone(),
                line: span.start().line,
                text: text.into(),
            },
        ));
    }
}

pub fn collect(root: &Path) -> Result<Vec<(&'static str, Hit)>> {
    let mut hits = Vec::new();
    for (rel, file) in read_sources(root)? {
        let mut visitor = crate::ast::Scanner::new(&rel);
        visitor.visit_file(&file);
        hits.extend(visitor.finish());
    }
    Ok(hits)
}

pub fn check(
    root: &Path,
    baseline: &crate::baseline::Baseline,
    name: &str,
    findings: &mut Vec<String>,
) -> Result<()> {
    let hits = collect(root)?
        .into_iter()
        .filter_map(|(check, hit)| (check == name).then_some(hit))
        .collect::<Vec<_>>();
    crate::baseline::adjudicate(baseline, name, &hits, findings);
    Ok(())
}
