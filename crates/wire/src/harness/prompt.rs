use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PromptKind {
    AskUser,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptQuestion {
    pub question: Option<String>,
    pub header: Option<String>,
    pub multi_select: bool,
    pub free_text: Option<bool>,
    pub options: Vec<PromptOption>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptOption {
    pub label: String,
    pub description: Option<String>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptResolution {
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub at: DateTime<Utc>,
    pub answer_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptScreen {
    pub current: u32,
    pub total: u32,
}
