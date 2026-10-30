use std::collections::BTreeMap;
use std::path::Path;

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::{self, Visit};

use crate::baseline::{Baseline, FILE};
use crate::error::Result;
use crate::git::tracked_files;
use crate::rustsrc::{Hit, is_scanned};

pub const CHECK: &str = "KERNELWIRE";
const TEXT: &str = "wire";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub file: String,
    pub line: usize,
    pub path: String,
}

impl Reference {
    pub fn hit(self) -> Hit {
        Hit {
            file: self.file,
            line: self.line,
            text: TEXT.to_owned(),
        }
    }
}

pub fn in_kernel(rel: &str) -> bool {
    rel.starts_with("crates/kernel/") && is_scanned(rel)
}

pub fn collect(root: &Path) -> Result<Vec<Reference>> {
    let mut references = Vec::new();
    for rel in tracked_files(root)?
        .into_iter()
        .filter(|rel| in_kernel(rel))
    {
        let text = std::fs::read_to_string(root.join(&rel))?;
        let mut scanner = Scanner {
            file: &rel,
            references: &mut references,
        };
        scanner.visit_file(&syn::parse_file(&text)?);
    }
    references.sort();
    Ok(references)
}

pub fn check(root: &Path, baseline: &Baseline, findings: &mut Vec<String>) -> Result<()> {
    let mut files: BTreeMap<String, Vec<Reference>> = BTreeMap::new();
    for reference in collect(root)? {
        files
            .entry(reference.file.clone())
            .or_default()
            .push(reference);
    }
    for (file, references) in &files {
        let allowed = baseline.count(CHECK, file, TEXT);
        if references.len() > allowed {
            findings.push(format!(
                "{CHECK} {file}: {} references to wire where the baseline allows {allowed} → docs/PATTERNS.md#kernelwire",
                references.len()
            ));
            findings.extend(references.iter().map(|reference| {
                format!(
                    "{CHECK} {}:{}: {}",
                    reference.file, reference.line, reference.path
                )
            }));
        }
    }
    for entry in baseline
        .entries()
        .iter()
        .filter(|entry| entry.check == CHECK)
    {
        let actual = files.get(&entry.file).map_or(0, Vec::len);
        if actual < entry.count {
            findings.push(format!(
                "BASELINE {FILE}: {CHECK} {} count {} no longer matches code ({actual}); reduce or remove the entry",
                entry.file, entry.count
            ));
        }
    }
    Ok(())
}

struct Scanner<'a> {
    file: &'a str,
    references: &'a mut Vec<Reference>,
}

impl Scanner<'_> {
    fn push(&mut self, span: proc_macro2::Span, path: String) {
        self.references.push(Reference {
            file: self.file.to_owned(),
            line: span.start().line,
            path,
        });
    }

    fn tokens(&mut self, stream: TokenStream) {
        let trees = stream.into_iter().collect::<Vec<_>>();
        for (index, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(group) => self.tokens(group.stream()),
                TokenTree::Ident(ident) if ident == TEXT && separator(&trees, index + 1) => {
                    self.push(ident.span(), token_path(&trees, index));
                }
                TokenTree::Literal(literal) => {
                    if let syn::Lit::Str(text) = syn::Lit::new(literal.clone())
                        && text.value().starts_with("wire::")
                    {
                        self.push(literal.span(), text.value());
                    }
                }
                TokenTree::Ident(_) | TokenTree::Punct(_) => {}
            }
        }
    }
}

fn separator(trees: &[TokenTree], index: usize) -> bool {
    matches!(
        (trees.get(index), trees.get(index + 1)),
        (Some(TokenTree::Punct(first)), Some(TokenTree::Punct(second)))
            if first.as_char() == ':' && second.as_char() == ':'
    )
}

fn token_path(trees: &[TokenTree], start: usize) -> String {
    let mut segments = Vec::new();
    let mut index = start;
    while let Some(TokenTree::Ident(ident)) = trees.get(index) {
        segments.push(ident.to_string());
        if !separator(trees, index + 1) {
            break;
        }
        index += 3;
    }
    segments.join("::")
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if let syn::UseTree::Path(path) = &item.tree
            && path.ident == TEXT
        {
            self.push(path.ident.span(), render(&item.tree));
        }
        visit::visit_item_use(self, item);
    }

    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if let syn::Meta::List(list) = &attribute.meta {
            self.tokens(list.tokens.clone());
        }
        visit::visit_attribute(self, attribute);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
        visit::visit_macro(self, mac);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.len() > 1
            && let Some(first) = path.segments.first()
            && first.ident == TEXT
        {
            let text = path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            self.push(first.ident.span(), text);
        }
        visit::visit_path(self, path);
    }
}

fn render(tree: &syn::UseTree) -> String {
    match tree {
        syn::UseTree::Path(path) => format!("{}::{}", path.ident, render(&path.tree)),
        syn::UseTree::Name(name) => name.ident.to_string(),
        syn::UseTree::Rename(rename) => format!("{} as {}", rename.ident, rename.rename),
        syn::UseTree::Glob(_) => "*".to_owned(),
        syn::UseTree::Group(group) => format!(
            "{{{}}}",
            group
                .items
                .iter()
                .map(render)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
