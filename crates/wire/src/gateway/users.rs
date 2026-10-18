use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_OWNER_ID: &str = "owner";

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsersRegistry {
    pub owner_user_id: String,
    pub unmapped_is_owner: bool,
    pub local_stores: Option<bool>,
    pub users: BTreeMap<String, UserRecord>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "B11: the users registry default requires the named owner id; derive would use an empty id"
    )
)]
impl Default for UsersRegistry {
    fn default() -> Self {
        Self {
            owner_user_id: DEFAULT_OWNER_ID.to_owned(),
            unmapped_is_owner: false,
            local_stores: None,
            users: BTreeMap::new(),
        }
    }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    pub id: Option<String>,
    pub devices: Option<Vec<String>>,
    pub pg_url: Option<String>,
    pub env_file: Option<String>,
    pub persona: Option<String>,
}
