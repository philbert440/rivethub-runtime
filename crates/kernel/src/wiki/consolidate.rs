use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::builder::WikiStoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsolidateWiki {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_limit_clusters")]
    pub limit_clusters: u32,
    #[serde(default = "default_min_cluster_size")]
    pub min_cluster_size: u32,
}

const fn default_limit_clusters() -> u32 {
    50
}

const fn default_min_cluster_size() -> u32 {
    2
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiFold {
    pub cluster: String,
    pub parent: String,
    pub children: Vec<String>,
}

#[async_trait]
pub trait WikiConsolidateStore: Send + Sync {
    async fn wiki_slugs(&self) -> Result<Vec<String>, WikiStoreError>;

    async fn fold_wiki(
        &self,
        fold: &WikiFold,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError>;
}

#[must_use]
pub fn consolidation_plan(slugs: &[String], options: &ConsolidateWiki) -> Vec<WikiFold> {
    let mut clusters: Vec<(String, Vec<String>)> = stem_clusters(slugs)
        .into_iter()
        .filter(|(_, members)| {
            !u32::try_from(members.len()).is_ok_and(|size| size < options.min_cluster_size)
        })
        .collect();
    clusters.sort_by(|left, right| {
        right
            .1
            .len()
            .cmp(&left.1.len())
            .then_with(|| left.0.cmp(&right.0))
    });
    clusters
        .into_iter()
        .zip(0..options.limit_clusters)
        .filter_map(|((cluster, members), _)| fold_for(cluster, &members))
        .collect()
}

fn fold_for(cluster: String, members: &[String]) -> Option<WikiFold> {
    let parent = canonical_parent(&cluster, members)?;
    let children: Vec<String> = members
        .iter()
        .filter(|slug| **slug != parent && is_slug_variant(slug, &parent))
        .cloned()
        .collect();
    (!children.is_empty()).then_some(WikiFold {
        cluster,
        parent,
        children,
    })
}

fn canonical_parent(cluster: &str, members: &[String]) -> Option<String> {
    if members.iter().any(|slug| slug == cluster) {
        return Some(cluster.to_owned());
    }
    members
        .iter()
        .min_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)))
        .cloned()
}

fn stem_clusters(slugs: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for slug in slugs.iter().filter(|slug| is_normalized(slug)) {
        let parts: Vec<&str> = slug.split('-').collect();
        let key = match parts.as_slice() {
            [first, second, _, ..] => format!("{first}-{second}"),
            _ => slug.clone(),
        };
        groups.entry(key).or_default().push(slug.clone());
    }
    groups
}

#[must_use]
pub fn is_slug_variant(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() || left == right {
        return left == right && !left.is_empty();
    }
    hyphen_child(left, right) || hyphen_child(right, left)
}

fn hyphen_child(child: &str, parent: &str) -> bool {
    child
        .strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('-'))
}

fn is_normalized(slug: &str) -> bool {
    !slug.is_empty()
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}
