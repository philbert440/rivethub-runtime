use chrono::{DateTime, Utc};
use wire::events::{Activity, AgentEventKind, SessionLogLine, SessionSummary, SessionView};

#[derive(Debug, Clone)]
pub(super) struct Projection {
    pub(super) summary: SessionSummary,
    pub(super) view: SessionView,
    pub(super) revision: u64,
}

pub(super) fn empty_view() -> SessionView {
    SessionView {
        title: String::new(),
        activity: Activity::Idle,
        tool: None,
        tasks: Vec::new(),
        thought: String::new(),
        last_message: String::new(),
        log: Vec::new(),
        term: Vec::new(),
        ended: false,
    }
}

impl Projection {
    pub(super) fn new(id: String) -> Self {
        Self {
            summary: SessionSummary {
                name: id.clone(),
                id,
                harness: None,
                last_event_ts: None,
                pty: None,
                context_window: None,
                compact_at: None,
                context_source: None,
            },
            view: empty_view(),
            revision: 0,
        }
    }

    pub(super) fn apply(&mut self, kind: &AgentEventKind, now: DateTime<Utc>) {
        self.revision += 1;
        let ts = match kind {
            AgentEventKind::SessionStart {
                title,
                name,
                harness,
                ts,
            } => {
                self.view.ended = false;
                self.rename(title.as_ref(), name.as_ref());
                if harness.is_some() {
                    self.summary.harness = *harness;
                }
                ts
            }
            AgentEventKind::SessionEnd { ts } => {
                self.view.ended = true;
                self.view.activity = Activity::Sleeping;
                ts
            }
            AgentEventKind::MessageUser { text, ts } => {
                self.view.thought.clear();
                self.view.tool = None;
                self.log("user", text);
                ts
            }
            AgentEventKind::MessageAgent { text, ts, .. } => {
                self.view.activity = Activity::Speaking;
                self.view.last_message.clone_from(text);
                self.view.tool = None;
                self.log("agent", text);
                ts
            }
            AgentEventKind::ThinkingDelta { text, ts } => {
                self.view.activity = Activity::Thinking;
                self.view.thought.push_str(text);
                self.view.thought = self
                    .view
                    .thought
                    .chars()
                    .rev()
                    .take(8_000)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                ts
            }
            AgentEventKind::ToolStart { tool, ts, .. } => {
                self.view.tool = Some(tool.clone());
                self.view.activity = Activity::EditingCode;
                ts
            }
            AgentEventKind::TaskPlan { tasks, ts } => {
                self.view.tasks.clone_from(tasks);
                ts
            }
            AgentEventKind::TaskCheck { index, ts } => {
                if let Some(task) = self.view.tasks.get_mut(*index as usize) {
                    *task = format!("✓ {task}");
                }
                ts
            }
            AgentEventKind::Activity { activity, ts } => {
                self.view.activity = *activity;
                ts
            }
            AgentEventKind::TurnEnd { ts } => {
                self.view.activity = Activity::Idle;
                self.view.tool = None;
                ts
            }
        };
        self.summary.last_event_ts = Some(match ts {
            Some(ts) => *ts,
            None => now,
        });
    }

    fn log(&mut self, who: &str, text: &str) {
        self.view.log.push(SessionLogLine {
            who: who.into(),
            text: text.chars().take(8_000).collect(),
        });
        if self.view.log.len() > 100 {
            self.view.log.remove(0);
        }
    }

    fn rename(&mut self, title: Option<&String>, name: Option<&String>) {
        if let Some(title) = title {
            self.view.title.clone_from(title);
        }
        if let Some(name) = name {
            self.summary.name.clone_from(name);
        }
    }
}
