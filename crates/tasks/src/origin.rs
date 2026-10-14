use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskOrigin {
    Heartbeat,
    Chat,
    Tool,
    Mesh,
    Api,
    Eval,
}
