use super::*;

#[tokio::test]
async fn a_permanent_status_still_retries_until_the_rest_revives_it() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(4));
    let row = recorded.tables.summaries.first().expect("summary");
    let store = Arc::new(MemStore::new(vec![window(
        row.conversation_id,
        "cli:s1",
        "Session cli:s1",
        messages(row.earliest_at, 10),
    )]));
    let script = Arc::new(Scripted::fail_with(vec![400, 400, 400]));
    let jobs = Arc::new(Seen::with_dead(vec![
        (
            JobName::Fixed(FixedJob::CompactConversation),
            at("2026-10-04T12:30:00Z"),
        ),
        (
            JobName::Fixed(FixedJob::EmbedTarget),
            at("2026-10-04T12:30:00Z"),
        ),
    ]));
    let settings = settings();
    let token = cancel();
    let payload = payload_of(row.conversation_id, SummaryTrigger::Window);
    for _ in 0..3 {
        let env = bind(
            &store,
            &script,
            &jobs,
            &settings,
            at("2026-10-04T12:10:00Z"),
            &token,
        );
        let error = handle_summarize(&env, &payload).await.expect_err("400");
        assert_eq!(error.to_string(), "LLM HTTP 400 (not retrying)");
    }
    assert!(
        recorded
            .logs
            .iter()
            .any(|line| line.contains("LLM HTTP 400 (not retrying)"))
    );
    assert_eq!(store.written.lock().await.len(), 0);
    let within = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:50:00Z"),
        &token,
    );
    assert_eq!(handle_reap(&within).await.expect("within rest"), 0);
    assert_eq!(jobs.dead_count(FixedJob::CompactConversation).await, 1);
    let after = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T13:51:00Z"),
        &token,
    );
    assert_eq!(handle_reap(&after).await.expect("after rest"), 1);
    assert_eq!(jobs.dead_count(FixedJob::CompactConversation).await, 0);
    assert_eq!(jobs.dead_count(FixedJob::EmbedTarget).await, 1);
    let outcome = handle_summarize(&after, &payload).await.expect("revived");
    assert_eq!(outcome.summaries, 1);
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
}

fn floor_windows(recorded: &Scenario) -> Vec<ConversationWindow> {
    let floor = recorded
        .tables
        .summaries
        .iter()
        .find(|row| row.message_count == 6)
        .expect("floor");
    let few = recorded
        .tables
        .summaries
        .iter()
        .find(|row| row.message_count == 3)
        .expect("few");
    vec![
        window(
            floor.conversation_id,
            "cli:floor",
            "Session cli:floor",
            messages(floor.earliest_at, 6),
        ),
        window(
            few.conversation_id,
            "cli:few",
            "Session cli:few",
            messages(few.earliest_at, 3),
        ),
    ]
}

#[tokio::test]
async fn idle_and_stale_floors_follow_the_recording() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(5));
    let store = Arc::new(MemStore::new(floor_windows(recorded)));
    let script = Arc::new(Scripted::success());
    let jobs = Arc::new(Seen::default());
    let settings = settings();
    let token = cancel();
    let early = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:09:00Z"),
        &token,
    );
    assert_eq!(handle_enqueue_idle(&early).await.expect("early idle"), 0);
    assert_eq!(handle_enqueue_stale(&early).await.expect("early stale"), 0);
    assert_health(
        &store,
        &settings,
        health_steps(recorded).first().expect("early health"),
    )
    .await;
    let idle = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:25:00Z"),
        &token,
    );
    assert_eq!(handle_enqueue_idle(&idle).await.expect("idle"), 1);
    let job = jobs.only().await;
    assert_eq!(read_trigger(&job.payload), SummaryTrigger::SessionIdle);
    let outcome = handle_summarize(&idle, &job.payload)
        .await
        .expect("idle compact");
    assert_eq!(outcome.summaries, 1);
    let stale = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-08T12:25:00Z"),
        &token,
    );
    assert_eq!(handle_enqueue_idle(&stale).await.expect("later idle"), 0);
    assert_eq!(handle_enqueue_stale(&stale).await.expect("stale"), 1);
    let job = jobs.only().await;
    assert_eq!(read_trigger(&job.payload), SummaryTrigger::Stale);
    let outcome = handle_summarize(&stale, &job.payload)
        .await
        .expect("stale compact");
    assert_eq!(outcome.summaries, 1);
    assert_calls(&script.calls.lock().await, &chat_calls(recorded));
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
    assert_health(
        &store,
        &settings,
        health_steps(recorded).get(1).expect("later health"),
    )
    .await;
    assert_eq!(summarized_counts(&recorded.logs), vec![1, 1]);
}

#[test]
fn summary_settings_reject_an_empty_batch() {
    let error = SummarySettings::from_parts(SummaryParts {
        model: "fake-chat".to_owned(),
        leaf_batch: 0,
        branch_batch: 8,
        root_batch: 5,
        min_leaves_for_branch: 5,
        min_branches_for_root: 3,
        idle_minutes: 15,
        stale_minutes: 5_760,
        stale_min_batch: 2,
        max_leaf_rounds: 10,
        pipeline_version: 5,
        session_tagging: false,
    })
    .expect_err("empty batch");
    assert!(error.to_string().contains("leaf batch"));
    assert_eq!(settings().idle_floor(), 5);
}

#[tokio::test]
async fn a_leaf_enqueues_suggest_tags_when_tagging_is_on() {
    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000002").expect("id");
    let store = Arc::new(MemStore::new(vec![window(
        id,
        "cli:s1",
        "Session cli:s1",
        messages(at("2026-10-10T12:00:00Z"), 10),
    )]));
    let script = Arc::new(Scripted::success());
    let jobs = Arc::new(Seen::default());
    let settings = tagging_settings();
    let token = cancel();
    let env = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-10T12:10:00Z"),
        &token,
    );
    let outcome = handle_summarize(&env, &payload_of(id, SummaryTrigger::Window))
        .await
        .expect("compact");
    assert_eq!(outcome.summaries, 1);
    assert!(!outcome.reenqueued);
    let job = jobs.only().await;
    let summary_id = store.written.lock().await.first().expect("leaf").id;
    let key = format!("tags-{summary_id}");
    assert_eq!(job.name, JobName::Fixed(FixedJob::SuggestTags));
    assert_eq!(job.key.as_deref(), Some(key.as_str()));
    assert_eq!(job.key_mode, JobKeyMode::Replace);
    assert_eq!(job.priority, 6);
    assert!(job.run_at.is_none());
    assert_tag_payload(&job, summary_id, id);
}
