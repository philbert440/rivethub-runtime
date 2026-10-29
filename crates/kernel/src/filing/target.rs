use std::str::FromStr;

use uuid::Uuid;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

use crate::memory::ConversationHead;

const SANDBOX_RUN_PREFIX: &str = "grokbot-run-sand-subagent-";
const HEARTBEAT_PREFIX: &str = "heartbeat";
const TASK_PREFIX: &str = "task:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
enum UnfiledChannel {
    TestChannel,
    EnvTestChannel,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Session(SessionId),
    Conversation(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filable {
    Session(SessionId),
    Task(Uuid),
    Conversation,
    Never,
}

impl Filable {
    #[must_use]
    pub fn of(head: &ConversationHead) -> Self {
        let key = head.session_key.as_str();
        if key.starts_with(SANDBOX_RUN_PREFIX)
            || key.starts_with(HEARTBEAT_PREFIX)
            || UnfiledChannel::from_str(&head.channel).is_ok()
        {
            return Self::Never;
        }
        if let Some(task) = key.strip_prefix(TASK_PREFIX) {
            #[cfg_attr(
                dylint_lib = "rr_lints",
                expect(
                    fallback,
                    reason = "PATTERNS FALLBACK: a task key without a task id files as a conversation"
                )
            )]
            let Ok(task) = Uuid::parse_str(task) else {
                return Self::Conversation;
            };
            return Self::Task(task);
        }
        if bare_claude_session(head) {
            return Self::Session(SessionId::new(HarnessId::ClaudeCode, key));
        }
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                fallback,
                reason = "PATTERNS FALLBACK: a key that is not a session id files as a conversation"
            )
        )]
        let Ok(session) = SessionId::from_str(key) else {
            return Self::Conversation;
        };
        Self::Session(session)
    }

    #[must_use]
    pub fn target(&self, conversation: Uuid) -> Option<Target> {
        match self {
            Self::Session(session) => Some(Target::Session(session.clone())),
            Self::Task(_) | Self::Conversation => Some(Target::Conversation(conversation)),
            Self::Never => None,
        }
    }
}

#[must_use]
pub fn conversation_keys(session: &SessionId) -> Vec<String> {
    let mut keys = vec![session.as_str().to_owned()];
    if session.harness() == HarnessId::ClaudeCode && Uuid::parse_str(session.native()).is_ok() {
        keys.push(session.native().to_owned());
    }
    keys
}

fn bare_claude_session(head: &ConversationHead) -> bool {
    HarnessId::from_str(&head.channel).is_ok_and(|harness| harness == HarnessId::ClaudeCode)
        && Uuid::parse_str(&head.session_key).is_ok()
}
