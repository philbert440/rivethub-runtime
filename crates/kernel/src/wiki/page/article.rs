use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::lines::fence_aware;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArticleSection {
    pub heading: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArticleSections {
    pub lead: String,
    pub sections: Vec<ArticleSection>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ArticlePatchMode {
    Replace,
    #[default]
    Append,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArticlePatch {
    pub heading: String,
    pub body: String,
    #[serde(default)]
    pub mode: ArticlePatchMode,
}

#[must_use]
pub fn demote_h2_headings(text: &str) -> String {
    fence_aware(text)
        .map(|line| match line.text.strip_prefix("## ") {
            Some(heading) if !line.fenced && !heading.starts_with('#') => format!("### {heading}"),
            _ => line.text.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[must_use]
pub fn split_article_sections(text: &str) -> ArticleSections {
    let text = text.replace("\r\n", "\n");
    let mut lead = Vec::new();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for line in fence_aware(&text) {
        match line.text.strip_prefix("### ") {
            Some(heading) if !line.fenced && !heading.is_empty() => {
                sections.push((heading.trim().to_owned(), Vec::new()));
            }
            _ => match sections.last_mut() {
                Some((_, body)) => body.push(line.text),
                None => lead.push(line.text),
            },
        }
    }
    ArticleSections {
        lead: lead.join("\n").trim().to_owned(),
        sections: sections
            .into_iter()
            .map(|(heading, body)| ArticleSection {
                heading,
                body: body.join("\n").trim().to_owned(),
            })
            .collect(),
    }
}

#[must_use]
pub fn join_article_sections(lead: &str, sections: &[ArticleSection]) -> String {
    let lead = lead.trim();
    let mut parts = Vec::with_capacity(sections.len() + 1);
    if !lead.is_empty() {
        parts.push(lead.to_owned());
    }
    parts.extend(
        sections
            .iter()
            .map(|section| format!("### {}\n\n{}", section.heading, section.body.trim())),
    );
    parts.join("\n\n").trim().to_owned()
}

#[must_use]
pub fn apply_article_patches(article: &str, patches: &[ArticlePatch]) -> String {
    let ArticleSections { lead, mut sections } = split_article_sections(article);
    for patch in patches {
        let heading = patch.heading.trim();
        let body = patch.body.trim();
        if heading.is_empty() || body.is_empty() {
            continue;
        }
        let existing = sections
            .iter_mut()
            .find(|section| section.heading.eq_ignore_ascii_case(heading));
        match (existing, patch.mode) {
            (Some(section), ArticlePatchMode::Append) if !section.body.is_empty() => {
                section.body = format!("{}\n\n{body}", section.body);
            }
            (Some(section), _) => body.clone_into(&mut section.body),
            (None, _) => sections.push(ArticleSection {
                heading: heading.to_owned(),
                body: body.to_owned(),
            }),
        }
    }
    join_article_sections(&lead, &sections)
}
