use std::future::Future;
use std::sync::Arc;

use agent_client_protocol as acp;
use kernel::harness::SessionEvent;
use tokio::sync::mpsc;

use crate::events;
use crate::permissions::PermissionRegistry;

pub(crate) struct ClientBridge {
    pub(crate) events: mpsc::Sender<SessionEvent>,
    pub(crate) permissions: Arc<PermissionRegistry>,
}

fn unsupported<T>() -> impl Future<Output = Result<T, acp::Error>> {
    std::future::ready(Err(acp::Error::method_not_found()))
}

impl acp::Client for ClientBridge {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        let (request_id, answer) = self.permissions.register(request.options.clone());
        let event = match events::permission_request(&request_id, &request) {
            Ok(event) => event,
            Err(error) => {
                self.permissions.withdraw(&request_id);
                return Err(acp::Error::internal_error().with_data(error.to_string()));
            }
        };
        if self
            .events
            .send(kernel::harness::SessionEvent::Permission(event))
            .await
            .is_err()
        {
            self.permissions.withdraw(&request_id);
            return Err(acp::Error::internal_error().with_data("session event stream is closed"));
        }
        let outcome = answer.await.map_err(|_| {
            acp::Error::internal_error().with_data("the permission request was withdrawn")
        })?;
        Ok(acp::RequestPermissionResponse {
            outcome,
            meta: None,
        })
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
        match events::session_event(notification.update) {
            Ok(event) => {
                if self.events.send(event).await.is_err() {
                    Err(acp::Error::internal_error().with_data("session event stream is closed"))
                } else {
                    Ok(())
                }
            }
            Err(error) => {
                tracing::warn!(%error, "session update has no session-event form; dropped");
                Ok(())
            }
        }
    }

    fn write_text_file(
        &self,
        _request: acp::WriteTextFileRequest,
    ) -> impl Future<Output = Result<acp::WriteTextFileResponse, acp::Error>> {
        unsupported()
    }

    fn read_text_file(
        &self,
        _request: acp::ReadTextFileRequest,
    ) -> impl Future<Output = Result<acp::ReadTextFileResponse, acp::Error>> {
        unsupported()
    }

    fn create_terminal(
        &self,
        _request: acp::CreateTerminalRequest,
    ) -> impl Future<Output = Result<acp::CreateTerminalResponse, acp::Error>> {
        unsupported()
    }

    fn terminal_output(
        &self,
        _request: acp::TerminalOutputRequest,
    ) -> impl Future<Output = Result<acp::TerminalOutputResponse, acp::Error>> {
        unsupported()
    }

    fn release_terminal(
        &self,
        _request: acp::ReleaseTerminalRequest,
    ) -> impl Future<Output = Result<acp::ReleaseTerminalResponse, acp::Error>> {
        unsupported()
    }

    fn wait_for_terminal_exit(
        &self,
        _request: acp::WaitForTerminalExitRequest,
    ) -> impl Future<Output = Result<acp::WaitForTerminalExitResponse, acp::Error>> {
        unsupported()
    }

    fn kill_terminal_command(
        &self,
        _request: acp::KillTerminalCommandRequest,
    ) -> impl Future<Output = Result<acp::KillTerminalCommandResponse, acp::Error>> {
        unsupported()
    }

    fn ext_method(
        &self,
        _method: Arc<str>,
        _params: Arc<serde_json::value::RawValue>,
    ) -> impl Future<Output = Result<Arc<serde_json::value::RawValue>, acp::Error>> {
        unsupported()
    }

    fn ext_notification(
        &self,
        _method: Arc<str>,
        _params: Arc<serde_json::value::RawValue>,
    ) -> impl Future<Output = Result<(), acp::Error>> {
        unsupported()
    }
}
