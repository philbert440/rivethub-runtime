use serde::{Deserialize, Serialize};

use super::{LastVerified, PageSource, WikiMeta, WikiSectionRef};
use values::Extra;

use super::WikiSlug;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct StoredFrontmatter {
    pub title: Option<String>,
    pub slug: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    pub last_verified: Option<LastVerified>,
    #[serde(default)]
    pub sources: Vec<PageSource>,
    #[serde(default)]
    pub sections: Vec<WikiSectionRef>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct WrittenFrontmatter<'a> {
    #[serde(flatten)]
    pub extra: &'a Extra,
    pub title: &'a str,
    pub slug: &'a WikiSlug,
    pub aliases: &'a [String],
    pub tags: &'a [String],
    pub entities: &'a [String],
    pub related: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_verified: Option<&'a LastVerified>,
    pub sources: &'a [PageSource],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub sections: &'a [WikiSectionRef],
}

impl<'a> WrittenFrontmatter<'a> {
    pub(super) fn new(meta: &'a WikiMeta, related: &'a [String]) -> Self {
        Self {
            extra: &meta.extra,
            title: &meta.title,
            slug: &meta.slug,
            aliases: &meta.aliases,
            tags: &meta.tags,
            entities: &meta.entities,
            related,
            last_verified: meta.last_verified.as_ref(),
            sources: &meta.sources,
            sections: &meta.sections,
        }
    }
}
