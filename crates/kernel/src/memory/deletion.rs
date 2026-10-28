use std::collections::BTreeMap;
use std::ops::AddAssign;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use wire::gateway::session_delete::SessionTrashReason;
use wire::gateway::sessions::HarnessTranscriptTurn;
use wire::gateway::sessions::SessionMessageRole;
use wire::session_id::SessionId;

use super::MemoryStoreError;

pub const TRASH_MIN_AGE: TimeDelta = TimeDelta::hours(1);
const TRASH_REPLY_MAX_CHARS: usize = 80;
/// A prompt longer than this carries real content, so its session is kept
/// even when the reply was a single word.
const TRASH_PROMPT_MAX_CHARS: usize = 280;
pub const TOMBSTONE_PURGE_BATCH: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeletedSession {
    pub conversations: u64,
    pub messages: u64,
    pub summaries: u64,
}

impl AddAssign for DeletedSession {
    fn add_assign(&mut self, other: Self) {
        self.conversations += other.conversations;
        self.messages += other.messages;
        self.summaries += other.summaries;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionActivity {
    pub conversations: u64,
    pub messages: u64,
    pub user_messages: u64,
    pub assistant_messages: u64,
    pub reply: Option<String>,
    pub tool_rows: u64,
    pub attachments: u64,
    pub tasked: bool,
    pub placed: bool,
    pub cited: bool,
}

impl SessionActivity {
    #[must_use]
    pub fn recorded(&self) -> bool {
        self.conversations > 0 || self.messages > 0
    }

    #[must_use]
    pub fn kept(&self) -> bool {
        self.tool_rows > 0 || self.attachments > 0 || self.tasked || self.placed || self.cited
    }
}

#[async_trait]
pub trait SessionDeletion: Send + Sync {
    async fn delete_session(
        &self,
        session: &SessionId,
        deleted_by: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<DeletedSession, MemoryStoreError>;

    async fn tombstones(
        &self,
        sessions: &[SessionId],
    ) -> Result<BTreeMap<SessionId, DateTime<Utc>>, MemoryStoreError>;

    async fn session_activity(
        &self,
        sessions: &[SessionId],
    ) -> Result<BTreeMap<SessionId, SessionActivity>, MemoryStoreError>;

    async fn purge_tombstoned(&self, limit: u32) -> Result<DeletedSession, MemoryStoreError>;
}

#[must_use]
pub fn used_since(
    deleted_at: DateTime<Utc>,
    stamps: impl IntoIterator<Item = Option<DateTime<Utc>>>,
) -> bool {
    stamps.into_iter().flatten().any(|stamp| stamp > deleted_at)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(ascii_case_insensitive)]
enum PlaceholderTitle {
    #[strum(serialize = "untitled")]
    Untitled,
    #[strum(serialize = "new session")]
    NewSession,
    #[strum(serialize = "new chat")]
    NewChat,
    #[strum(serialize = "new conversation")]
    NewConversation,
    #[strum(serialize = "dsh session")]
    DshSession,
    #[strum(serialize = "claude code session")]
    ClaudeCodeSession,
}

#[must_use]
pub fn placeholder_title(title: Option<&str>, native: &str) -> bool {
    let Some(title) = title.map(str::trim) else {
        return true;
    };
    title.is_empty() || title == native || title.parse::<PlaceholderTitle>().is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashSignals<'a> {
    pub native: &'a str,
    pub title: Option<&'a str>,
    pub updated_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
    pub activity: &'a SessionActivity,
    pub transcript: &'a [HarnessTranscriptTurn],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashVerdict {
    pub reason: SessionTrashReason,
    pub user_turns: u64,
    pub reply: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Turns {
    user: u64,
    assistant: u64,
    reply: Option<String>,
    prompt_chars: usize,
    kept: bool,
}

impl Turns {
    fn of_transcript(transcript: &[HarnessTranscriptTurn]) -> Self {
        let mut turns = Self::default();
        for turn in transcript {
            let tools = turn.tools.as_ref().is_some_and(|tools| !tools.is_empty());
            let attachments = turn
                .attachments
                .as_ref()
                .is_some_and(|attachments| !attachments.is_empty());
            turns.kept |= tools || attachments;
            match turn.role {
                SessionMessageRole::User => {
                    let prompt = turn.text.trim();
                    if !prompt.is_empty() {
                        turns.user += 1;
                        turns.prompt_chars = turns.prompt_chars.max(prompt.chars().count());
                    }
                }
                SessionMessageRole::Assistant => {
                    turns.assistant += 1;
                    if turns.reply.is_none() {
                        turns.reply = Some(turn.text.clone());
                    }
                }
            }
        }
        turns
    }

    const fn silent(&self) -> bool {
        self.user == 0 && self.assistant == 0
    }

    fn brief(&self) -> bool {
        self.user <= 1 && self.assistant <= 1 && self.reply.as_deref().is_none_or(one_word)
    }

    fn of_memory(activity: &SessionActivity) -> Self {
        Self {
            user: activity.user_messages,
            assistant: activity.assistant_messages,
            reply: activity.reply.clone(),
            prompt_chars: 0,
            kept: activity.kept(),
        }
    }
}

fn one_word(text: &str) -> bool {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation());
    !trimmed.chars().any(char::is_whitespace)
}

fn preview(reply: Option<&str>) -> Option<String> {
    let text = reply.map(str::trim).filter(|text| !text.is_empty())?;
    Some(text.chars().take(TRASH_REPLY_MAX_CHARS).collect())
}

#[must_use]
pub fn trash_verdict(signals: &TrashSignals<'_>) -> Option<TrashVerdict> {
    if signals.now.signed_duration_since(signals.updated_at) < TRASH_MIN_AGE {
        return None;
    }
    let memory = Turns::of_memory(signals.activity);
    let transcript = Turns::of_transcript(signals.transcript);
    if memory.kept || transcript.kept {
        return None;
    }
    if memory.silent() && transcript.silent() {
        return placeholder_title(signals.title, signals.native).then_some(TrashVerdict {
            reason: SessionTrashReason::Empty,
            user_turns: 0,
            reply: None,
        });
    }
    let user_turns = memory.user.max(transcript.user);
    let reply = transcript.reply.as_deref().or(memory.reply.as_deref());
    // An unanswered prompt or a long one is not obviously junk: the user may
    // still want to retry it, or the prompt itself was the content.
    let answered = reply.is_some_and(|reply| !reply.trim().is_empty());
    let brief = user_turns == 1
        && answered
        && transcript.prompt_chars <= TRASH_PROMPT_MAX_CHARS
        && memory.brief()
        && transcript.brief();
    brief.then(|| TrashVerdict {
        reason: SessionTrashReason::OneWordReply,
        user_turns,
        reply: preview(reply),
    })
}
