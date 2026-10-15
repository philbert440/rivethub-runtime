use wire::gateway::memory::MemoryHealthResponse;
use wire::gateway::memory::SummarizerHealth;

#[test]
fn summarizer_health_preserves_the_recorded_field_name() -> Result<(), serde_json::Error> {
    let recorded = serde_json::json!({
        "compaction": {"eligible": true, "activeTail": false, "belowFloor": false},
        "status": "ok",
        "embeddings": {"status": "ok"},
        "embedQueueDepth": 0
    });
    let health: MemoryHealthResponse = serde_json::from_value(recorded.clone())?;
    assert_eq!(
        health.summarizer,
        Some(SummarizerHealth {
            eligible: true,
            active_tail: false,
            below_floor: false,
        })
    );
    assert_eq!(serde_json::to_value(health)?, recorded);
    Ok(())
}
