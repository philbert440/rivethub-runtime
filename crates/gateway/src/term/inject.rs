use std::time::Duration;

use kernel::terminal::TerminalId;
use wire::gateway::term::{PtyState, TermInjectRequest};

use super::TerminalService;
use crate::error::ApiError;
use crate::tenancy::UserId;

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
const SUBMIT: &[u8] = b"\r";
const INTERRUPT: &[u8] = b"\x1b";
const SUBMIT_DELAY: Duration = Duration::from_millis(80);
const INTERRUPT_SETTLE: Duration = Duration::from_millis(400);

impl TerminalService {
    pub(crate) async fn inject(
        &self,
        user: &UserId,
        request: TermInjectRequest,
    ) -> Result<String, ApiError> {
        let (_, handle) = self.find(user, None, Some(&request.session))?;
        let state = handle
            .state
            .borrow()
            .clone()
            .filter(|state| state.state == PtyState::Running)
            .ok_or_else(|| ApiError::Conflict("no live terminal for this session".into()))?;
        let id = TerminalId::new(&state.id);
        let _turn = self.injecting.lock().await;
        if request.interrupt == Some(true) {
            self.mux.input(&id, INTERRUPT.to_vec()).await?;
            tokio::time::sleep(INTERRUPT_SETTLE).await;
        }
        let text = request.text.replace('\u{1b}', "");
        if request.submit == Some(false) {
            self.mux.input(&id, text.into_bytes()).await?;
            return Ok(state.id);
        }
        let mut pasted = PASTE_START.to_vec();
        pasted.extend_from_slice(text.as_bytes());
        pasted.extend_from_slice(PASTE_END);
        self.mux.input(&id, pasted).await?;
        tokio::time::sleep(SUBMIT_DELAY).await;
        self.mux.input(&id, SUBMIT.to_vec()).await?;
        Ok(state.id)
    }
}
