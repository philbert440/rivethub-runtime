use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersRegistry {
    pub owner_user_id: String,
    pub unmapped_is_owner: bool,
    pub local_stores: Option<bool>,
    pub users: BTreeMap<String, UserRecord>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    pub id: Option<String>,
    pub devices: Option<Vec<String>>,
    pub pg_url: Option<String>,
    pub env_file: Option<String>,
    pub persona: Option<String>,
}
