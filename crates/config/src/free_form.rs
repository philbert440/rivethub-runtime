use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(transparent)]
pub struct FreeForm(pub BTreeMap<String, serde_yaml_ng::Value>);
