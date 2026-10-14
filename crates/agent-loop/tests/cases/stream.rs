use std::time::Duration;

use agent_loop::Error;
use kernel::model::ModelChunk;
use tokio::sync::mpsc;
use wire::JsonObject;
use wire::message::{MessageContent, Role, ToolCall};

#[tokio::test]
async fn interleaved_argument_fragments_are_parsed_into_objects() {
    let sequence = vec![
        ModelChunk::ToolCallStart {
            index: 1,
            id: "second".to_owned(),
            name: "echo".to_owned(),
        },
        ModelChunk::ToolCallStart {
            index: 0,
            id: "first".to_owned(),
            name: "echo".to_owned(),
        },
        ModelChunk::ToolCallDelta {
            index: 1,
            arguments_delta: r#"{"text":"🦀"#.to_owned(),
        },
        ModelChunk::ToolCallDelta {
            index: 0,
            arguments_delta: r#"{"count":1,"text":"first"}"#.to_owned(),
        },
        ModelChunk::Reasoning {
            delta: "consider".to_owned(),
        },
        ModelChunk::ToolCallDelta {
            index: 1,
            arguments_delta: r#"","count":2}"#.to_owned(),
        },
        ModelChunk::Done,
    ];
    let (input, stream) = mpsc::channel(sequence.len());
    for chunk in &sequence {
        input.send(Ok(chunk.clone())).await.expect("source channel");
    }
    drop(input);
    let (output, mut events) = mpsc::channel(sequence.len());
    let result = agent_loop::stream::collect(stream, &output, 1024)
        .await
        .expect("assembled");
    assert_eq!(result.message.role, Role::Assistant);
    assert_eq!(result.message.content, MessageContent::Text(String::new()));
    assert_eq!(result.message.reasoning.as_deref(), Some("consider"));
    let first: JsonObject =
        serde_json::from_str(r#"{"text":"first","count":1}"#).expect("first arguments");
    let second: JsonObject =
        serde_json::from_str(r#"{"count":2,"text":"🦀"}"#).expect("second arguments");
    assert_eq!(
        result.message.tool_calls,
        Some(vec![
            ToolCall {
                id: "first".to_owned(),
                name: "echo".to_owned(),
                arguments: first,
                thought_signature: None
            },
            ToolCall {
                id: "second".to_owned(),
                name: "echo".to_owned(),
                arguments: second,
                thought_signature: None
            },
        ])
    );
    drop(output);
    let mut received = Vec::new();
    while let Some(event) = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("event deadline")
    {
        received.push(event);
    }
    assert_eq!(received, sequence);
}

#[tokio::test]
async fn duplicate_ids_and_empty_tool_identity_are_rejected() {
    for (first_id, second_id, tool) in [
        ("same", "same", "echo"),
        ("", "second", "echo"),
        ("first", "second", ""),
    ] {
        let (input, stream) = mpsc::channel(3);
        for (index, id) in [(0, first_id), (1, second_id)] {
            input
                .send(Ok(ModelChunk::ToolCallStart {
                    index,
                    id: id.to_owned(),
                    name: tool.to_owned(),
                }))
                .await
                .expect("source");
        }
        input.send(Ok(ModelChunk::Done)).await.expect("done");
        let (output, _events) = mpsc::channel(3);
        assert!(matches!(
            agent_loop::stream::collect(stream, &output, 1024).await,
            Err(Error::Stream(_))
        ));
    }
}
