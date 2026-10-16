use config::Environment;

use crate::WorkerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryJobSettings {
    pub sweep_limit: u32,
    pub sweep_max_attempts: u32,
}

impl MemoryJobSettings {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            sweep_limit: 100,
            sweep_max_attempts: 5,
        }
    }

    pub fn from_environment(env: &Environment) -> Result<Self, WorkerError> {
        let mut settings = Self::standard();
        for (name, slot) in [
            ("RIVETHUB_EMBED_SWEEP_LIMIT", &mut settings.sweep_limit),
            (
                "RIVETHUB_EMBED_SWEEP_MAX_ATTEMPTS",
                &mut settings.sweep_max_attempts,
            ),
        ] {
            if let Some(value) = env.get(name) {
                let text = value.to_str().ok_or_else(|| WorkerError::JobSetting {
                    name: name.to_owned(),
                })?;
                let parsed = text.parse::<u32>().map_err(|_| WorkerError::JobSetting {
                    name: name.to_owned(),
                })?;
                if parsed == 0 || parsed > i32::MAX as u32 {
                    return Err(WorkerError::JobSetting {
                        name: name.to_owned(),
                    });
                }
                *slot = parsed;
            }
        }
        Ok(settings)
    }
}
