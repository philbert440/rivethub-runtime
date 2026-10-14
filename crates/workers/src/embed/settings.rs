use std::time::Duration;

use config::Config;

use crate::error::WorkerError;

const EMBEDDING_WIDTH: u32 = 1024;
const CHARS_PER_CHUNK: u32 = 6_000;
const TOOL_RESULT_CAP: u32 = 32_768;
const MAX_NULL_FAILURES: u32 = 3;
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedSettings {
    pub width: u32,
    pub chars_per_chunk: u32,
    pub tool_result_cap: u32,
    pub max_failures: u32,
    pub chunks_enabled: bool,
    pub timeout: Duration,
}

impl EmbedSettings {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            width: EMBEDDING_WIDTH,
            chars_per_chunk: CHARS_PER_CHUNK,
            tool_result_cap: TOOL_RESULT_CAP,
            max_failures: MAX_NULL_FAILURES,
            chunks_enabled: true,
            timeout: EMBED_TIMEOUT,
        }
    }

    pub fn from_config(config: &Config) -> Result<Self, WorkerError> {
        let mut settings = Self::standard();
        if let Some(width) = postgres_width(config) {
            if width == 0 {
                return Err(WorkerError::Width);
            }
            settings.width = width;
        }
        Ok(settings)
    }

    #[must_use]
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_chunks_enabled(mut self, enabled: bool) -> Self {
        self.chunks_enabled = enabled;
        self
    }
}

fn postgres_width(config: &Config) -> Option<u32> {
    config
        .memory
        .as_ref()
        .and_then(|memory| memory.postgres.as_ref())
        .and_then(|postgres| postgres.embed_expected_dims)
}
