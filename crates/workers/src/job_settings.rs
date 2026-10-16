use config::Environment;

use crate::WorkerError;

const WIKI_SWEEP_CEILING: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryJobSettings {
    pub sweep_limit: u32,
    pub chunk_backfill_limit: u32,
    pub sweep_max_attempts: u32,
    pub wiki_sweep_limit: u32,
}

impl MemoryJobSettings {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            sweep_limit: 200,
            chunk_backfill_limit: 200,
            sweep_max_attempts: 5,
            wiki_sweep_limit: 20,
        }
    }

    pub fn from_environment(env: &Environment) -> Result<Self, WorkerError> {
        let mut settings = Self::standard();
        for (name, slot) in [
            ("RIVETHUB_EMBED_SWEEP_LIMIT", &mut settings.sweep_limit),
            (
                "RIVETHUB_CHUNK_BACKFILL_LIMIT",
                &mut settings.chunk_backfill_limit,
            ),
            (
                "RIVETHUB_EMBED_SWEEP_MAX_ATTEMPTS",
                &mut settings.sweep_max_attempts,
            ),
        ] {
            if let Some(parsed) = number(env, name)? {
                if parsed == 0 || parsed > i64::from(i32::MAX) {
                    return Err(WorkerError::JobSetting {
                        name: name.to_owned(),
                    });
                }
                *slot = u32::try_from(parsed).map_err(|_| WorkerError::JobSetting {
                    name: name.to_owned(),
                })?;
            }
        }
        let name = "RIVETHUB_WIKI_SWEEP_LIMIT";
        if let Some(parsed) = number(env, name)? {
            settings.wiki_sweep_limit =
                u32::try_from(parsed.clamp(0, i64::from(WIKI_SWEEP_CEILING))).map_err(|_| {
                    WorkerError::JobSetting {
                        name: name.to_owned(),
                    }
                })?;
        }
        Ok(settings)
    }
}

pub fn worker_concurrency(env: &Environment) -> Result<Option<u32>, WorkerError> {
    let name = "RIVETHUB_WORKER_CONCURRENCY";
    let invalid = || WorkerError::JobSetting {
        name: name.to_owned(),
    };
    match number(env, name)? {
        None => Ok(None),
        Some(parsed) if parsed > 0 => u32::try_from(parsed).map(Some).map_err(|_| invalid()),
        Some(_) => Err(invalid()),
    }
}

fn number(env: &Environment, name: &str) -> Result<Option<i64>, WorkerError> {
    let invalid = || WorkerError::JobSetting {
        name: name.to_owned(),
    };
    env.get(name)
        .map(|value| {
            value
                .to_str()
                .ok_or_else(invalid)?
                .parse::<i64>()
                .map_err(|_| invalid())
        })
        .transpose()
}
