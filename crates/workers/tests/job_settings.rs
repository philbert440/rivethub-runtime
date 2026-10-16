#![cfg(test)]

use config::Environment;
use workers::{EmbedSettings, MemoryJobSettings, WorkerError};

fn with(name: &str, value: &str) -> Result<MemoryJobSettings, WorkerError> {
    MemoryJobSettings::from_environment(&Environment::from_vars([(name, value)]))
}

#[test]
fn unconfigured_sweeps_use_the_datahub_defaults() -> Result<(), WorkerError> {
    let expected = MemoryJobSettings {
        sweep_limit: 200,
        chunk_backfill_limit: 200,
        sweep_max_attempts: 5,
        wiki_sweep_limit: 20,
    };
    assert_eq!(MemoryJobSettings::standard(), expected);
    assert_eq!(
        MemoryJobSettings::from_environment(&Environment::default())?,
        expected
    );
    assert!(EmbedSettings::standard().chunks_enabled);
    assert_eq!(EmbedSettings::standard().chars_per_chunk, 6000);
    Ok(())
}

#[test]
fn environment_overrides_are_typed_and_the_wiki_cap_is_clamped() -> Result<(), WorkerError> {
    assert_eq!(with("RIVETHUB_EMBED_SWEEP_LIMIT", "50")?.sweep_limit, 50);
    assert_eq!(
        with("RIVETHUB_CHUNK_BACKFILL_LIMIT", "75")?.chunk_backfill_limit,
        75
    );
    assert_eq!(
        with("RIVETHUB_EMBED_SWEEP_MAX_ATTEMPTS", "9")?.sweep_max_attempts,
        9
    );
    assert_eq!(
        with("RIVETHUB_WIKI_SWEEP_LIMIT", "500")?.wiki_sweep_limit,
        200
    );
    assert_eq!(with("RIVETHUB_WIKI_SWEEP_LIMIT", "-4")?.wiki_sweep_limit, 0);
    assert_eq!(with("RIVETHUB_WIKI_SWEEP_LIMIT", "0")?.wiki_sweep_limit, 0);
    assert_eq!(
        with("RIVETHUB_WIKI_SWEEP_LIMIT", "35")?.wiki_sweep_limit,
        35
    );
    for (name, value) in [
        ("RIVETHUB_WIKI_SWEEP_LIMIT", "many"),
        ("RIVETHUB_CHUNK_BACKFILL_LIMIT", "0"),
        ("RIVETHUB_EMBED_SWEEP_LIMIT", "-1"),
        ("RIVETHUB_EMBED_SWEEP_MAX_ATTEMPTS", "3000000000"),
    ] {
        assert!(
            matches!(with(name, value), Err(WorkerError::JobSetting { .. })),
            "{name}={value}"
        );
    }
    Ok(())
}

#[test]
fn worker_concurrency_is_unset_by_default_and_must_be_a_positive_count() -> Result<(), WorkerError>
{
    let concurrency = |value: &str| {
        workers::worker_concurrency(&Environment::from_vars([(
            "RIVETHUB_WORKER_CONCURRENCY",
            value,
        )]))
    };
    assert_eq!(workers::worker_concurrency(&Environment::default())?, None);
    assert_eq!(concurrency("2")?, Some(2));
    for refused in ["0", "-1", "two", "4294967296"] {
        assert!(
            matches!(concurrency(refused), Err(WorkerError::JobSetting { ref name }) if name == "RIVETHUB_WORKER_CONCURRENCY"),
            "{refused}"
        );
    }
    Ok(())
}
