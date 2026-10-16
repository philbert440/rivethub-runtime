use kernel::memory::jobs::{MemoryJobStore, SynthesizeToolCall};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use wire::JsonObject;
use wire::message::MessageContent;
use workers::tool_synthesis::{TOOL_SYNTH_SYSTEM_PROMPT, handle_synthesize_tool_call};

use super::support::{TestResult, message_id, response, seed};

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn synthesis_writes_valid_content_and_passes_the_exact_request(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    sqlx::query("UPDATE rh_messages SET content = $1")
        .bind("\n\t")
        .execute(&pool)
        .await?;
    let provider = response("  Read the configuration at /etc/service.conf.  ");
    let payload = JsonObject::from_serialize(&SynthesizeToolCall {
        message_id: message_id(),
    })?;
    handle_synthesize_tool_call(
        &store,
        &provider,
        "summary-model",
        &payload,
        &CancellationToken::new(),
    )
    .await?;
    let row = store.tool_call(message_id()).await?.expect("message");
    assert_eq!(row.content, "Read the configuration at /etc/service.conf.");
    let requests = provider.chat_requests().await;
    let request = requests.first().expect("request");
    assert_eq!(request.model, "summary-model");
    assert_eq!(request.max_tokens, Some(200));
    assert_eq!(request.options.temperature, Some(0.2));
    assert_eq!(request.options.enable_thinking, Some(false));
    assert_eq!(
        request.messages.first().expect("system").content,
        MessageContent::Text(TOOL_SYNTH_SYSTEM_PROMPT.to_owned())
    );
    assert_eq!(
        request.messages.get(1).expect("user").content,
        MessageContent::Text(
            "tool_name: read\ntool_args: {\"path\":\"/etc/service.conf\"}\n".to_owned()
        )
    );
    handle_synthesize_tool_call(
        &store,
        &provider,
        "summary-model",
        &payload,
        &CancellationToken::new(),
    )
    .await?;
    assert_eq!(provider.chat_requests().await.len(), 1);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn synthesis_drops_missing_populated_and_nameless_rows(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    let provider = response("Unused model response.");
    for (id, content, name) in [
        (uuid::Uuid::nil(), "", Some("read")),
        (message_id(), "Existing content.", Some("read")),
        (message_id(), "  ", None),
    ] {
        sqlx::query("UPDATE rh_messages SET content = $2, tool_name = $3 WHERE id = $1")
            .bind(id)
            .bind(content)
            .bind(name)
            .execute(&pool)
            .await?;
        let payload = JsonObject::from_serialize(&SynthesizeToolCall { message_id: id })?;
        handle_synthesize_tool_call(
            &store,
            &provider,
            "model",
            &payload,
            &CancellationToken::new(),
        )
        .await?;
    }
    assert_eq!(provider.chat_requests().await, Vec::new());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn synthesis_rejects_bad_answers_and_counts_unicode_characters(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    let payload = JsonObject::from_serialize(&SynthesizeToolCall {
        message_id: message_id(),
    })?;
    let long = format!("{}.", "a".repeat(500));
    for answer in [
        "",
        "Ran it.",
        "The ASSISTANT read the file.",
        "Read the file without punctuation",
        &long,
        "😀😀😀😀.",
    ] {
        let error = handle_synthesize_tool_call(
            &store,
            &response(answer),
            "model",
            &payload,
            &CancellationToken::new(),
        )
        .await
        .expect_err("invalid");
        assert_eq!(error.to_string(), "Empty synth response");
        assert_eq!(
            store
                .tool_call(message_id())
                .await?
                .expect("message")
                .content,
            ""
        );
    }
    let unicode = format!("{}.", "😀".repeat(499));
    handle_synthesize_tool_call(
        &store,
        &response(&unicode),
        "model",
        &payload,
        &CancellationToken::new(),
    )
    .await?;
    assert_eq!(
        store
            .tool_call(message_id())
            .await?
            .expect("message")
            .content,
        unicode
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn synthesis_parses_encoded_arguments_and_refuses_wrong_types(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    for args in [
        r#""{\"path\":\"/etc/service.conf\"}""#,
        r#"{"path":"/etc/service.conf"}"#,
    ] {
        sqlx::query("UPDATE rh_messages SET tool_args = $1::jsonb")
            .bind(args)
            .execute(&pool)
            .await?;
        assert_eq!(
            store
                .tool_call(message_id())
                .await?
                .expect("message")
                .tool_args,
            serde_json::from_str::<JsonObject>(r#"{"path":"/etc/service.conf"}"#)?
        );
    }
    for args in ["null", "\"\""] {
        sqlx::query("UPDATE rh_messages SET tool_args = $1::jsonb")
            .bind(args)
            .execute(&pool)
            .await?;
        assert_eq!(
            store
                .tool_call(message_id())
                .await?
                .expect("message")
                .tool_args,
            JsonObject::default()
        );
    }
    for args in ["42", "[]", "true", "\"bad json\""] {
        sqlx::query("UPDATE rh_messages SET tool_args = $1::jsonb")
            .bind(args)
            .execute(&pool)
            .await?;
        assert!(store.tool_call(message_id()).await.is_err());
    }
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn synthesis_does_not_overwrite_concurrent_content(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    sqlx::query("UPDATE rh_messages SET content = 'Concurrent content.'")
        .execute(&pool)
        .await?;
    store
        .write_tool_sentence(message_id(), "", "Synthesized sentence.")
        .await?;
    assert_eq!(
        store
            .tool_call(message_id())
            .await?
            .expect("message")
            .content,
        "Concurrent content."
    );
    Ok(())
}
