use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Hook(#[from] crate::hooks::HookError),
    #[error(transparent)]
    Provider(#[from] kernel::model::ModelProviderError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
    #[error("invalid model stream: {0}")]
    Stream(String),
    #[error("hook pipeline refused the operation")]
    Refused,
    #[error("turn cancelled")]
    Cancelled,
    #[error("turn timed out")]
    Timeout,
    #[error("stream consumer disconnected")]
    Disconnected,
    #[error("duplicate tool definition: {0}")]
    DuplicateTool(String),
    #[error("turn requires a final user message")]
    MissingUserMessage,
    #[error("token usage overflow")]
    UsageOverflow,
    #[error("elapsed duration overflow")]
    DurationOverflow,
}
