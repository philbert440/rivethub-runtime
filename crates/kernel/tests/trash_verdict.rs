use chrono::{DateTime, TimeDelta, Utc};
use kernel::memory::{SessionActivity, TrashSignals, placeholder_title, trash_verdict};
use wire::gateway::session_delete::SessionTrashReason;
use wire::gateway::sessions::{HarnessTranscriptTurn, SessionMessageRole};

fn turn(role: SessionMessageRole, text: &str) -> HarnessTranscriptTurn {
    HarnessTranscriptTurn {
        role,
        text: text.to_owned(),
        thinking: None,
        tools: None,
        usage: None,
        model: None,
        stop_reason: None,
        last_block: None,
        attachments: None,
        complete: false,
        compact: false,
    }
}

fn now() -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + TimeDelta::days(20_000)
}

fn verdict(
    activity: &SessionActivity,
    transcript: &[HarnessTranscriptTurn],
    title: Option<&str>,
    age: TimeDelta,
) -> Option<(SessionTrashReason, u64, Option<String>)> {
    trash_verdict(&TrashSignals {
        native: "native-1",
        title,
        updated_at: now() - age,
        now: now(),
        activity,
        transcript,
    })
    .map(|verdict| (verdict.reason, verdict.user_turns, verdict.reply))
}

#[test]
fn placeholder_titles_are_the_id_blank_or_a_harness_default() {
    for title in [
        None,
        Some(""),
        Some("  "),
        Some("native-1"),
        Some("Untitled"),
        Some("new SESSION"),
        Some("dsh session"),
    ] {
        assert!(placeholder_title(title, "native-1"), "{title:?}");
    }
    assert!(!placeholder_title(Some("Fix the deploy"), "native-1"));
}

#[test]
fn an_old_silent_session_with_a_default_title_is_empty() {
    let quiet = SessionActivity::default();
    assert_eq!(
        verdict(&quiet, &[], None, TimeDelta::hours(2)),
        Some((SessionTrashReason::Empty, 0, None))
    );
    assert_eq!(
        verdict(&quiet, &[], Some("Plan"), TimeDelta::hours(2)),
        None
    );
    assert_eq!(verdict(&quiet, &[], None, TimeDelta::minutes(59)), None);
    assert_eq!(verdict(&quiet, &[], None, -TimeDelta::hours(2)), None);
}

#[test]
fn a_one_word_reply_is_trimmed_of_punctuation_and_previewed() {
    let transcript = [
        turn(SessionMessageRole::User, "say hi"),
        turn(SessionMessageRole::Assistant, "  Hi!  "),
    ];
    assert_eq!(
        verdict(
            &SessionActivity::default(),
            &transcript,
            None,
            TimeDelta::hours(2)
        ),
        Some((SessionTrashReason::OneWordReply, 1, Some("Hi!".to_owned())))
    );
    let unanswered = [turn(SessionMessageRole::User, "hello?")];
    assert_eq!(
        verdict(
            &SessionActivity::default(),
            &unanswered,
            None,
            TimeDelta::hours(2)
        ),
        Some((SessionTrashReason::OneWordReply, 1, None))
    );
}

#[test]
fn any_doubt_keeps_the_session() {
    let two_words = [
        turn(SessionMessageRole::User, "hi"),
        turn(SessionMessageRole::Assistant, "Hello there"),
    ];
    assert_eq!(
        verdict(
            &SessionActivity::default(),
            &two_words,
            None,
            TimeDelta::hours(2)
        ),
        None
    );
    let longer = [
        turn(SessionMessageRole::User, "hi"),
        turn(SessionMessageRole::Assistant, "Hi"),
        turn(SessionMessageRole::User, "and then"),
    ];
    assert_eq!(
        verdict(
            &SessionActivity::default(),
            &longer,
            None,
            TimeDelta::hours(2)
        ),
        None
    );
    let memory_has_more = SessionActivity {
        conversations: 1,
        messages: 4,
        user_messages: 2,
        assistant_messages: 2,
        reply: Some("Sure".to_owned()),
        ..SessionActivity::default()
    };
    assert_eq!(
        verdict(&memory_has_more, &[], None, TimeDelta::hours(2)),
        None
    );
    let transcript_has_more = SessionActivity {
        conversations: 1,
        messages: 2,
        user_messages: 1,
        assistant_messages: 1,
        reply: Some("Sure".to_owned()),
        ..SessionActivity::default()
    };
    assert_eq!(
        verdict(&transcript_has_more, &longer, None, TimeDelta::hours(2)),
        None
    );
    for kept in [
        SessionActivity {
            tool_rows: 1,
            ..SessionActivity::default()
        },
        SessionActivity {
            attachments: 1,
            ..SessionActivity::default()
        },
        SessionActivity {
            placed: true,
            ..SessionActivity::default()
        },
        SessionActivity {
            cited: true,
            ..SessionActivity::default()
        },
        SessionActivity {
            tasked: true,
            ..SessionActivity::default()
        },
    ] {
        assert_eq!(
            verdict(&kept, &[], None, TimeDelta::hours(2)),
            None,
            "{kept:?}"
        );
    }
}
