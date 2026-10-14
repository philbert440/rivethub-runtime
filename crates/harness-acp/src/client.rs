use agent_client_protocol::schema::v1 as acp;
use kernel::harness::SessionEvent;
use tokio::sync::mpsc;

use crate::events;
use crate::permissions::PermissionRegistry;

pub(crate) async fn on_session_update(
    notification: acp::SessionNotification,
    events: &mpsc::Sender<SessionEvent>,
) -> Result<(), acp::Error> {
    match events::session_event(notification.update) {
        Ok(event) => {
            if events.send(event).await.is_err() {
                Err(acp::Error::internal_error().data("session event stream is closed"))
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

pub(crate) async fn on_permission(
    request: acp::RequestPermissionRequest,
    responder: agent_client_protocol::Responder<acp::RequestPermissionResponse>,
    permissions: &PermissionRegistry,
    events: &mpsc::Sender<SessionEvent>,
) -> Result<(), acp::Error> {
    let (request_id, answer) = permissions.register(request.options.clone());
    let event = match events::permission_request(&request_id, &request) {
        Ok(event) => event,
        Err(error) => {
            permissions.withdraw(&request_id);
            return responder
                .respond_with_error(acp::Error::internal_error().data(error.to_string()));
        }
    };
    if events.send(SessionEvent::Permission(event)).await.is_err() {
        permissions.withdraw(&request_id);
        return responder.respond_with_error(
            acp::Error::internal_error().data("session event stream is closed"),
        );
    }
    let outcome = match answer.await {
        Ok(outcome) => outcome,
        Err(error) => {
            return responder.respond_with_error(
                acp::Error::internal_error()
                    .data(format!("the permission request was withdrawn: {error}")),
            );
        }
    };
    responder.respond(acp::RequestPermissionResponse::new(outcome))
}
