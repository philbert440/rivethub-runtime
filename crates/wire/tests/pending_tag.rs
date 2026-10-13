use chrono::DateTime;
use chrono::Utc;
use wire::gateway::memory::PendingTagAction;
use wire::gateway::memory::PendingTagWire;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagState;
use wire::gateway::memory::TagWire;

fn tag_wire() -> TagWire {
    TagWire {
        id: "00000000-0000-4000-8000-000000000001".to_string(),
        entity_type: TagEntityType::Conversation,
        entity_id: "00000000-0000-4000-8000-000000000002".to_string(),
        key: "project".to_string(),
        value: "acme".to_string(),
        display: "Acme".to_string(),
        source: "model".to_string(),
        state: TagState::Suggested,
        confidence: None,
        proposed_by: "tagger".to_string(),
        reason: "recurring topic".to_string(),
        decided_by: None,
        decided_at: None,
        created_at: DateTime::<Utc>::UNIX_EPOCH,
        updated_at: DateTime::<Utc>::UNIX_EPOCH,
    }
}

fn pending_tag(action: Option<PendingTagAction>) -> PendingTagWire {
    PendingTagWire {
        tag: tag_wire(),
        action,
        session_key: Some("claude-code:abc".to_string()),
        title: Some("AcmeKit rollout".to_string()),
        agent: Some("agent-a".to_string()),
        conversation_id: Some("00000000-0000-4000-8000-000000000003".to_string()),
        excerpt: None,
    }
}

#[test]
fn pending_tag_action_serde_names_match_the_spec() {
    let expected = [
        (PendingTagAction::Add, "\"add\""),
        (PendingTagAction::Remove, "\"remove\""),
    ];
    for (action, json) in expected {
        assert_eq!(serde_json::to_string(&action).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<PendingTagAction>(json).unwrap(),
            action
        );
    }
    assert!(serde_json::from_str::<PendingTagAction>("\"keep\"").is_err());
}

#[test]
fn pending_tag_wire_roundtrips_with_and_without_an_action() {
    for action in [
        Some(PendingTagAction::Add),
        Some(PendingTagAction::Remove),
        None,
    ] {
        let pending = pending_tag(action);
        let json = serde_json::to_value(&pending).unwrap();
        assert_eq!(
            json.get("action"),
            action.map(|a| serde_json::json!(a)).as_ref()
        );
        let back: PendingTagWire = serde_json::from_value(json).unwrap();
        assert_eq!(back, pending);
    }
}

#[test]
fn pending_tag_wire_accepts_a_payload_without_an_action() {
    let mut json = serde_json::to_value(pending_tag(Some(PendingTagAction::Remove))).unwrap();
    json.as_object_mut()
        .unwrap()
        .remove("action")
        .expect("expected an action member");
    let parsed: PendingTagWire = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.action, None);
    assert_eq!(parsed.tag.key, "project");
}
