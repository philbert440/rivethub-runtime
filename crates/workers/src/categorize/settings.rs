use crate::error::WorkerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSettings {
    pub model: String,
    pub allow_protected_removals: bool,
}

impl TagSettings {
    pub fn new(
        model: impl Into<String>,
        allow_protected_removals: bool,
    ) -> Result<Self, WorkerError> {
        let model = model.into();
        if model.is_empty() {
            return Err(WorkerError::TagSettings {
                message: "tag model is empty".to_owned(),
            });
        }
        Ok(Self {
            model,
            allow_protected_removals,
        })
    }
}
