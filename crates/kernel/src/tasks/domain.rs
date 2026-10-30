use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

pub const MAX_DEPTH: u8 = 3;
pub const LIVENESS: Duration = Duration::from_secs(30);
pub const REAP_AFTER_SECS: i64 = 90;
pub const UNCLAIMED_PAST_DEADLINE: &str = "no node started it before its maxWallClockMs ran out";

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        duptype,
        reason = "KERNELWIRE: the kernel owns a task's lifecycle; wire::task::TaskStatus is the contract's copy, translated by the gateway's task routes"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    Running,
    AwaitingInput,
    Completed,
    Failed,
    Killed,
    Timeout,
}

impl TaskStatus {
    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Killed | Self::Timeout
        )
    }

    #[must_use]
    pub const fn can_become(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Queued,
                Self::Running | Self::Failed | Self::Killed | Self::Timeout
            ) | (Self::Running, Self::AwaitingInput)
                | (Self::AwaitingInput, Self::Running)
                | (
                    Self::Running | Self::AwaitingInput,
                    Self::Completed | Self::Failed | Self::Killed | Self::Timeout
                )
        )
    }
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        duptype,
        reason = "KERNELWIRE: the kernel owns how a task ended; wire::task::TaskVerdict is the contract's copy, translated by the gateway's task routes"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Completed,
    Failed,
    Killed,
    Timeout,
    BudgetExceeded,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Budget {
    pub wall_clock: Option<Duration>,
    pub max_turns: Option<u32>,
}

impl Budget {
    #[must_use]
    pub fn over_turns(&self, tool_steps: u32) -> bool {
        self.max_turns.is_some_and(|max| tool_steps > max)
    }
}

#[must_use]
pub fn stale_before(now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    now.checked_sub_signed(chrono::TimeDelta::try_seconds(REAP_AFTER_SECS)?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ParentSource {
    Launch,
    Request,
    Capture,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parent {
    pub session: Option<String>,
    pub task: Option<Uuid>,
    pub depth: u8,
    pub source: Option<ParentSource>,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("a delegated task may nest at most {MAX_DEPTH} deep")]
pub struct DepthError;

impl Parent {
    pub fn of(
        session: Option<String>,
        source: ParentSource,
        above: Option<&Task>,
    ) -> Result<Self, DepthError> {
        let Some(above) = above else {
            return Ok(Self {
                source: session.as_ref().map(|_| source),
                session,
                task: None,
                depth: 0,
            });
        };
        let depth = above.parent.depth.checked_add(1).ok_or(DepthError)?;
        if depth > MAX_DEPTH {
            return Err(DepthError);
        }
        Ok(Self {
            session,
            task: Some(above.id),
            depth,
            source: Some(source),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum Harness {
    ClaudeCode,
    GrokBuild,
    KimiCode,
    Hermes,
    Codex,
    Opencode,
    Omp,
    Dsh,
    Cursor,
    Cowork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub agent: String,
    pub preset: Option<String>,
    pub node: String,
    pub harness: Option<Harness>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub wall_clock_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub verdict: Verdict,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: Uuid,
    pub goal: String,
    pub criteria: Vec<Criterion>,
    pub budget: Budget,
    pub status: TaskStatus,
    pub target: Target,
    pub parent: Parent,
    pub child: Option<String>,
    pub result: Option<Outcome>,
    pub error: Option<String>,
    pub usage: Option<Usage>,
    pub requested_by: Option<String>,
    pub owner: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub alive_at: Option<DateTime<Utc>>,
}

impl Task {
    #[must_use]
    pub fn unclaimed_past_deadline(&self, now: DateTime<Utc>) -> bool {
        let waited = now
            .signed_duration_since(self.created_at)
            .num_milliseconds();
        self.status == TaskStatus::Queued
            && waited >= 0
            && self
                .budget
                .wall_clock
                .is_some_and(|wall| u128::from(waited.unsigned_abs()) >= wall.as_millis())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finish {
    pub status: TaskStatus,
    pub result: Option<Outcome>,
    pub error: Option<String>,
    pub usage: Option<Usage>,
    pub at: DateTime<Utc>,
}

impl Finish {
    #[must_use]
    pub fn failed(error: impl Into<String>, at: DateTime<Utc>) -> Self {
        Self {
            status: TaskStatus::Failed,
            result: None,
            error: Some(error.into()),
            usage: None,
            at,
        }
    }
}

#[must_use]
pub fn first_turn(task: &Task) -> String {
    let mut text = task.goal.clone();
    if !task.criteria.is_empty() {
        text.push_str("\n\nAcceptance criteria:");
        for criterion in &task.criteria {
            text.push_str("\n- ");
            text.push_str(&criterion.description);
        }
    }
    text.push_str("\n\nReply with a short summary of what you did and the result; it goes back to the session that asked (task ");
    text.push_str(&task.id.to_string());
    text.push_str(").");
    text
}

#[cfg(test)]
mod tests {
    use super::Budget;
    use super::DepthError;
    use super::MAX_DEPTH;
    use super::Parent;
    use super::ParentSource;
    use super::TaskStatus;

    #[test]
    fn terminal_statuses_are_final_and_running_flips_with_awaiting_input() {
        use TaskStatus::{AwaitingInput, Completed, Failed, Killed, Queued, Running, Timeout};
        let all = [
            Queued,
            Running,
            AwaitingInput,
            Completed,
            Failed,
            Killed,
            Timeout,
        ];
        for done in [Completed, Failed, Killed, Timeout] {
            assert!(all.iter().all(|next| !done.can_become(*next)));
        }
        assert!(Queued.can_become(Running));
        assert!(Running.can_become(AwaitingInput));
        assert!(AwaitingInput.can_become(Running));
        assert!(!Queued.can_become(Completed));
        assert!(!Queued.can_become(AwaitingInput));
        assert!(Running.can_become(Completed));
        assert!(AwaitingInput.can_become(Completed));
        for open in [Queued, Running, AwaitingInput] {
            for end in [Failed, Killed, Timeout] {
                assert!(open.can_become(end));
            }
        }
    }

    #[test]
    fn tool_steps_past_max_turns_exceed_the_budget() {
        let budget = Budget {
            wall_clock: None,
            max_turns: Some(2),
        };
        assert!(!budget.over_turns(2));
        assert!(budget.over_turns(3));
        assert!(!Budget::default().over_turns(u32::MAX));
    }

    #[test]
    fn nesting_stops_at_the_depth_cap() {
        let mut task = testkit_free_task();
        for depth in 0..MAX_DEPTH {
            task.parent.depth = depth;
            let parent = Parent::of(Some("s".into()), ParentSource::Launch, Some(&task));
            assert_eq!(parent.map(|parent| parent.depth), Ok(depth + 1));
        }
        task.parent.depth = MAX_DEPTH;
        assert_eq!(
            Parent::of(None, ParentSource::Request, Some(&task)),
            Err(DepthError)
        );
        let root = Parent::of(None, ParentSource::Request, None).expect("root");
        assert_eq!((root.depth, root.source), (0, None));
    }

    fn testkit_free_task() -> super::Task {
        super::Task {
            id: uuid::Uuid::nil(),
            goal: String::new(),
            criteria: Vec::new(),
            budget: Budget::default(),
            status: TaskStatus::Running,
            target: super::Target {
                agent: String::new(),
                preset: None,
                node: String::new(),
                harness: None,
                model: None,
                effort: None,
                cwd: None,
                permission_mode: None,
            },
            parent: Parent::default(),
            child: None,
            result: None,
            error: None,
            usage: None,
            requested_by: None,
            owner: String::new(),
            created_at: chrono::DateTime::UNIX_EPOCH,
            started_at: None,
            completed_at: None,
            alive_at: None,
        }
    }
}
