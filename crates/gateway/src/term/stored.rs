use std::path::PathBuf;
use std::sync::Arc;

use kernel::transcript::SessionLister;
use wire::harness::HarnessId;

use super::TerminalService;
use crate::error::ApiError;

impl TerminalService {
    #[must_use]
    pub fn with_session_lister(mut self, lister: Arc<dyn SessionLister>) -> Self {
        self.session_lister = Some(lister);
        self
    }

    pub(crate) async fn stored(
        &self,
        harness: HarnessId,
        native: &str,
    ) -> Result<Option<bool>, ApiError> {
        match &self.session_lister {
            Some(lister) => Ok(Some(
                lister.describe(harness, native, None).await?.is_some(),
            )),
            None => Ok(None),
        }
    }

    pub(crate) async fn recorded_cwd(
        &self,
        session: &wire::session_id::SessionId,
    ) -> Result<Option<PathBuf>, ApiError> {
        match &self.session_lister {
            Some(lister) => Ok(lister
                .store_cwd(session.harness(), session.native())
                .await?
                .map(PathBuf::from)
                .filter(|dir| dir.is_dir())),
            None => Ok(None),
        }
    }

    pub(crate) async fn resume(
        &self,
        harness: Option<HarnessId>,
        session: &str,
        resume: Option<String>,
    ) -> Result<Option<String>, ApiError> {
        if resume.is_some() {
            return Ok(resume);
        }
        match (&self.session_lister, harness) {
            (Some(lister), Some(harness))
                if wire::gateway::roster::session_flags(harness).is_some() =>
            {
                let stored = lister.describe(harness, session, None).await?;
                Ok(stored.map(|_| session.to_owned()))
            }
            _ => Ok(None),
        }
    }
}
