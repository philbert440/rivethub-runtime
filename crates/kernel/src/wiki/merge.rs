use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::page::WikiSlug;
use super::page::bound::MergeBase;
use crate::BoxError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeInput {
    pub title: String,
    pub summary: String,
    pub article: String,
    pub material: Vec<String>,
    pub limit: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub summary_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Merged {
    pub summary: String,
    pub article: String,
    pub history: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPage {
    pub slug: WikiSlug,
    pub base: MergeBase,
}

#[async_trait]
pub trait PageMerger: Send + Sync {
    async fn merge(&self, input: &MergeInput) -> Result<Merged, BoxError>;
}

#[must_use]
pub fn merge_schema() -> schemars::Schema {
    schemars::schema_for!(Merged)
}
