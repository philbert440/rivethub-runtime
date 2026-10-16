use std::collections::VecDeque;
use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use futures::stream;
use kernel::model::ModelChunk;
use wire::gateway::v1::{ChunkChoice, CompletionChunk, CompletionDelta};
use wire::message::Role;

use super::Error;
use super::chat::Running;

struct Stream {
    running: Running,
    pending: VecDeque<Event>,
    include_usage: bool,
    finished: bool,
}

pub(super) fn response(running: Running, include_usage: bool) -> Response {
    let state = Stream {
        running,
        pending: VecDeque::new(),
        include_usage,
        finished: false,
    };
    let initial = chunk(
        &state.running,
        CompletionDelta {
            role: Some(Role::Assistant),
            ..CompletionDelta::default()
        },
    );
    let first = Event::default().json_data(initial);
    let mut state = state;
    match first {
        Ok(event) => state.pending.push_back(event),
        Err(error) => return Error::Stream(error).into_response(),
    }
    let stream = stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok::<_, Infallible>(event), state));
            }
            if state.finished {
                return None;
            }
            match advance(&mut state).await {
                Ok(()) => {}
                Err(error) => {
                    state.finished = true;
                    error.record();
                    match Event::default().json_data(error.payload().1) {
                        Ok(event) => state.pending.push_back(event),
                        Err(error) => {
                            tracing::error!(%error, "cannot serialize stream error");
                            return None;
                        }
                    }
                }
            }
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn chunk(running: &Running, delta: CompletionDelta) -> CompletionChunk {
    CompletionChunk {
        id: running.id.clone(),
        created: running.created,
        model: running.model.clone(),
        choices: vec![ChunkChoice {
            index: 0,
            delta,
            finish_reason: None,
        }],
        usage: None,
    }
}

async fn advance(state: &mut Stream) -> Result<(), Error> {
    if let Some(event) = state.running.events.recv().await {
        let delta = match event {
            ModelChunk::Text { delta } => CompletionDelta {
                content: Some(delta),
                ..CompletionDelta::default()
            },
            ModelChunk::Reasoning { delta } => CompletionDelta {
                reasoning_content: Some(delta),
                ..CompletionDelta::default()
            },
            ModelChunk::Usage { .. } | ModelChunk::Done => return Ok(()),
            ModelChunk::ToolCallStart { .. } | ModelChunk::ToolCallDelta { .. } => {
                return Err(Error::Invalid(
                    "provider requested an unavailable tool".to_owned(),
                ));
            }
        };
        state
            .pending
            .push_back(Event::default().json_data(chunk(&state.running, delta))?);
        return Ok(());
    }
    let completion = (&mut state.running.result)
        .await
        .map_err(|_| Error::Unavailable)??;
    let mut final_chunk = chunk(&state.running, CompletionDelta::default());
    for choice in &mut final_chunk.choices {
        choice.finish_reason = completion
            .choices
            .first()
            .map(|choice| choice.finish_reason);
    }
    state
        .pending
        .push_back(Event::default().json_data(&final_chunk)?);
    if state.include_usage {
        final_chunk.choices.clear();
        final_chunk.usage = completion.usage;
        state
            .pending
            .push_back(Event::default().json_data(final_chunk)?);
    }
    state.pending.push_back(Event::default().data("[DONE]"));
    state.finished = true;
    Ok(())
}
