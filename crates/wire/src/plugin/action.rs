use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::JsonObject;
use crate::plugin::RouteName;
use crate::plugin::view::{ViewContext, ViewTone, ViewValue};
use crate::session_id::SessionId;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ViewAction {
    #[serde(rename_all = "camelCase")]
    Call {
        action: RouteName,
        params: Option<JsonObject>,
    },
    #[serde(rename_all = "camelCase")]
    Navigate {
        view: RouteName,
        params: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replace: bool,
    },
    #[serde(rename_all = "camelCase")]
    Dialog {
        view: RouteName,
        params: Option<JsonObject>,
    },
    Back,
    Refresh,
    #[serde(rename_all = "camelCase")]
    Toast {
        text: String,
        tone: Option<ViewTone>,
    },
    #[serde(rename_all = "camelCase")]
    Copy {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    OpenSession {
        session_id: SessionId,
    },
    #[serde(rename_all = "camelCase")]
    OpenTopic {
        topic_id: String,
    },
    #[serde(rename_all = "camelCase")]
    OpenUrl {
        url: Url,
    },
    #[serde(rename_all = "camelCase")]
    Download {
        path: String,
        filename: Option<String>,
    },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewActionRequest {
    pub params: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, ViewValue>,
    #[serde(default)]
    pub context: ViewContext,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewActionResult {
    #[serde(default)]
    pub actions: Vec<ViewAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewActionRefusal {
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}
