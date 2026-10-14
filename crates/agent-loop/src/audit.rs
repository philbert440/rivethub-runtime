use std::path::Path;

use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use wire::hooks::HookPipelineResult;

use crate::hooks::HookError;

#[derive(Debug)]
struct WriteRequest {
    bytes: Vec<u8>,
    reply: oneshot::Sender<Result<(), HookError>>,
}

#[derive(Debug, Clone)]
pub struct AuditLog {
    sender: mpsc::Sender<WriteRequest>,
}

impl AuditLog {
    pub async fn open(
        path: &Path,
        tasks: &mut JoinSet<Result<(), HookError>>,
        cancel: CancellationToken,
    ) -> Result<Self, HookError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let (sender, receiver) = mpsc::channel(32);
        tasks.spawn(write_records(file, receiver, cancel));
        Ok(Self { sender })
    }

    pub async fn record(&self, record: &HookPipelineResult) -> Result<(), HookError> {
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        let (reply, response) = oneshot::channel();
        self.sender
            .send(WriteRequest { bytes, reply })
            .await
            .map_err(|_| HookError::AuditClosed)?;
        response.await.map_err(|_| HookError::AuditClosed)?
    }
}

async fn write_records(
    mut file: File,
    mut receiver: mpsc::Receiver<WriteRequest>,
    cancel: CancellationToken,
) -> Result<(), HookError> {
    loop {
        let request = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                receiver.close();
                break;
            }
            request = receiver.recv() => request,
        };
        let Some(request) = request else { break };
        persist(&mut file, request).await?;
    }
    while let Some(request) = receiver.recv().await {
        persist(&mut file, request).await?;
    }
    Ok(())
}

async fn persist(file: &mut File, request: WriteRequest) -> Result<(), HookError> {
    file.write_all(&request.bytes).await?;
    file.flush().await?;
    file.sync_data().await?;
    if request.reply.send(Ok(())).is_err() {
        return Ok(());
    }
    Ok(())
}
