use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::task::TaskStatus;

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename = "task.done", rename_all = "camelCase")]
pub struct TaskDoneNotification {
    pub task_id: String,
    pub status: TaskStatus,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub ts: DateTime<Utc>,
}
