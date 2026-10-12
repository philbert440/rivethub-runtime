use std::str::FromStr;

use kernel::queue::JobName;
use kernel::queue::JobNameError;

const FIXED: [(&str, JobName); 15] = [
    ("embed-target", JobName::EmbedTarget),
    ("enqueue-unembedded", JobName::EnqueueUnembedded),
    ("enqueue-unchunked", JobName::EnqueueUnchunked),
    ("compact-conversation", JobName::CompactConversation),
    ("synthesize-tool-call", JobName::SynthesizeToolCall),
    ("suggest-tags", JobName::SuggestTags),
    ("enqueue-idle", JobName::EnqueueIdle),
    ("extract-wiki", JobName::ExtractWiki),
    ("enqueue-wiki-backfill", JobName::EnqueueWikiBackfill),
    ("consolidate-wiki", JobName::ConsolidateWiki),
    ("recompile-wiki", JobName::RecompileWiki),
    ("enqueue-stale-wiki", JobName::EnqueueStaleWiki),
    ("enqueue-stale-compaction", JobName::EnqueueStaleCompaction),
    ("reap-dead-jobs", JobName::ReapDeadJobs),
    ("run-task", JobName::RunTask),
];

#[test]
fn fixed_names_round_trip_through_the_wire_string() {
    for (wire, name) in FIXED {
        assert_eq!(name.to_string(), wire);
        assert_eq!(JobName::from_str(wire), Ok(name));
    }
}

#[test]
fn heartbeat_with_a_dashed_agent_round_trips() {
    let name = JobName::Heartbeat {
        agent: "rivet-edge-agent".to_string(),
        index: 7,
    };
    assert_eq!(name.to_string(), "heartbeat-rivet-edge-agent-7");
    assert_eq!(
        JobName::from_str("heartbeat-rivet-edge-agent-7"),
        Ok(name.clone())
    );
    assert_eq!(String::from(name), "heartbeat-rivet-edge-agent-7");
}

#[test]
fn run_task_on_a_node_round_trips() {
    let name = JobName::RunTaskOn {
        node: "node-b".to_string(),
    };
    assert_eq!(name.to_string(), "run-task:node-b");
    assert_eq!(JobName::from_str("run-task:node-b"), Ok(name.clone()));
    assert_eq!(String::from(name), "run-task:node-b");
}

#[test]
fn heartbeat_splits_at_the_last_dash() {
    assert_eq!(
        JobName::from_str("heartbeat-agent-10"),
        Ok(JobName::Heartbeat {
            agent: "agent".to_string(),
            index: 10
        })
    );
    assert_eq!(
        JobName::from_str("heartbeat-a-b-c-0"),
        Ok(JobName::Heartbeat {
            agent: "a-b-c".to_string(),
            index: 0
        })
    );
}

#[test]
fn rejects_an_unknown_name() {
    assert_eq!(
        JobName::from_str("no-such-job"),
        Err(JobNameError::Unknown("no-such-job".to_string()))
    );
    assert_eq!(
        JobName::from_str("heartbeat"),
        Err(JobNameError::Unknown("heartbeat".to_string()))
    );
}

#[test]
fn rejects_a_non_numeric_heartbeat_index() {
    assert_eq!(
        JobName::from_str("heartbeat-agent-x"),
        Err(JobNameError::HeartbeatIndex(
            "heartbeat-agent-x".to_string()
        ))
    );
    assert_eq!(
        JobName::from_str("heartbeat-agent"),
        Err(JobNameError::HeartbeatIndex("heartbeat-agent".to_string()))
    );
    assert_eq!(
        JobName::from_str("heartbeat-agent-1-x"),
        Err(JobNameError::HeartbeatIndex(
            "heartbeat-agent-1-x".to_string()
        ))
    );
}

#[test]
fn rejects_an_empty_run_task_node() {
    assert_eq!(JobName::from_str("run-task:"), Err(JobNameError::EmptyNode));
}

#[test]
fn serde_uses_the_wire_string() {
    let heartbeat = JobName::Heartbeat {
        agent: "edge-agent".to_string(),
        index: 3,
    };
    assert_eq!(
        serde_json::to_string(&heartbeat).unwrap(),
        "\"heartbeat-edge-agent-3\""
    );
    let run_task_on = JobName::RunTaskOn {
        node: "node-b".to_string(),
    };
    assert_eq!(
        serde_json::to_string(&run_task_on).unwrap(),
        "\"run-task:node-b\""
    );
    assert_eq!(
        serde_json::to_string(&JobName::RunTask).unwrap(),
        "\"run-task\""
    );
    for (wire, name) in FIXED {
        let parsed: JobName = serde_json::from_str(&serde_json::to_string(&name).unwrap()).unwrap();
        assert_eq!(parsed, name.clone());
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            format!("\"{wire}\"")
        );
    }
}
