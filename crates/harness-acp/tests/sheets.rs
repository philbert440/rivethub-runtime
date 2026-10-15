#![cfg(test)]

use std::path::{Path, PathBuf};

use config::Config;
use harness_acp::AcpHarnessAdapter;
use kernel::harness::HarnessAdapter;
use wire::harness::{HarnessId, HarnessModelsSource, HarnessOffering};

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn config_from(document: &str) -> Config {
    serde_yaml_ng::from_str(document).expect("config")
}

fn quote_path(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn offerings(document: &str) -> Result<Vec<HarnessOffering>, kernel::harness::HarnessAdapterError> {
    let config = config_from(document);
    let adapter = AcpHarnessAdapter::new(&config);
    block_on(adapter.offerings())
}

fn one(document: &str, harness: HarnessId) -> HarnessOffering {
    let listed = offerings(document).expect("offerings");
    let matches = listed
        .into_iter()
        .filter(|offering| offering.harness_id == harness)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one offering for {harness}");
    matches.into_iter().next().expect("offering")
}

#[test]
fn absent_binaries_and_unsupported_harnesses_are_omitted() {
    let listed = offerings("tasks: {}\n").expect("offerings");
    assert_eq!(listed, Vec::new());
    let directory = tempfile::tempdir().expect("directory");
    std::fs::write(directory.path().join("not-a-harness"), b"bin").expect("file");
    let listed = offerings(&format!(
        "tasks:\n  harnesses:\n    pi:\n      binary: {}\n    cursor:\n      binary: {}\n    cowork:\n      binary: {}\n",
        quote_path(&directory.path().join("not-a-harness")),
        quote_path(&directory.path().join("not-a-harness")),
        quote_path(&directory.path().join("not-a-harness")),
    ))
    .expect("offerings");
    assert_eq!(listed, Vec::new());
}

#[test]
fn a_verified_binary_publishes_the_static_claude_sheet() {
    let file = tempfile::NamedTempFile::new().expect("binary");
    let offering = one(
        &format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {}\n",
            quote_path(file.path())
        ),
        HarnessId::ClaudeCode,
    );
    assert!(offering.chat);
    assert!(offering.capabilities.control.interrupt);
    assert!(offering.capabilities.control.resume);
    assert!(offering.capabilities.control.approvals);
    assert!(offering.capabilities.observation.live_stream);
    assert!(!offering.capabilities.observation.list_sessions);
    assert_eq!(offering.capabilities.observation.drive, Some(true));
    assert_eq!(offering.capabilities.turn_options, Some(true));
    assert_eq!(offering.capabilities.launch_model, Some(true));
    assert_eq!(offering.capabilities.image_attachments, None);
    assert_eq!(offering.capabilities.model_flag.as_deref(), Some("--model"));
    assert_eq!(
        offering.capabilities.effort_flag.as_deref(),
        Some("--effort")
    );
    assert_eq!(
        offering.capabilities.models_source,
        Some(HarnessModelsSource::Static)
    );
    let ids = model_ids(&offering);
    assert_eq!(
        ids,
        vec![
            "fable",
            "opus",
            "sonnet",
            "haiku",
            "fable[1m]",
            "opus[1m]",
            "sonnet[1m]",
            "haiku[1m]",
        ]
    );
}

#[test]
fn an_unverified_binary_is_listed_without_chat() {
    let file = tempfile::NamedTempFile::new().expect("binary");
    let offering = one(
        &format!(
            "tasks:\n  harnesses:\n    grok-build:\n      binary: {}\n",
            quote_path(file.path())
        ),
        HarnessId::GrokBuild,
    );
    assert!(!offering.chat);
    assert!(!offering.capabilities.control.interrupt);
    assert_eq!(offering.capabilities.turn_options, None);
    assert_eq!(offering.capabilities.launch_model, None);
    assert_eq!(offering.capabilities.model_flag.as_deref(), Some("--model"));
    assert_eq!(
        offering.capabilities.effort_flag.as_deref(),
        Some("--reasoning-effort")
    );
    assert_eq!(
        offering.capabilities.models_source,
        Some(HarnessModelsSource::Static)
    );
    let models = offering.capabilities.models.expect("static floor");
    assert_eq!(models.len(), 1);
    let model = models.first().expect("floor");
    assert_eq!(model.id, "grok-4.6");
    assert_eq!(model.is_default, Some(true));
}

#[test]
fn discover_keeps_the_static_sheet_and_replace_merge_follow_the_mode() {
    let file = tempfile::NamedTempFile::new().expect("binary");
    let binary = quote_path(file.path());
    let discovered = one(
        &format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {binary}\n      models_mode: discover\n      models: ['../secret', 'custom/model']\n      efforts: [low]\n"
        ),
        HarnessId::ClaudeCode,
    );
    assert_eq!(
        discovered.capabilities.models_source,
        Some(HarnessModelsSource::Static)
    );
    assert!(model_ids(&discovered).contains(&"fable".to_owned()));
    assert!(!model_ids(&discovered).contains(&"custom/model".to_owned()));
    assert!(discovered.capabilities.efforts.is_none());

    let replaced = one(
        &format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {binary}\n      models_mode: replace\n      models: ['custom/model', 'custom/model', 'sonnet[1m]']\n      efforts: [low, high, low]\n"
        ),
        HarnessId::ClaudeCode,
    );
    assert_eq!(
        replaced.capabilities.models_source,
        Some(HarnessModelsSource::Config)
    );
    assert_eq!(
        model_ids(&replaced),
        vec!["custom/model".to_owned(), "sonnet[1m]".to_owned()]
    );
    let efforts = replaced.capabilities.efforts.expect("efforts");
    assert_eq!(
        efforts
            .iter()
            .map(|effort| effort.id.as_str())
            .collect::<Vec<_>>(),
        vec!["low", "high"]
    );

    let merged = one(
        &format!(
            "tasks:\n  harnesses:\n    hermes:\n      binary: {binary}\n      models: ['vendor/model']\n      efforts: [medium]\n"
        ),
        HarnessId::Hermes,
    );
    assert!(!merged.chat);
    assert_eq!(merged.capabilities.model_flag.as_deref(), Some("-m"));
    assert_eq!(
        merged.capabilities.effort_flag.as_deref(),
        Some("--reasoning")
    );
    assert_eq!(
        merged.capabilities.models_source,
        Some(HarnessModelsSource::Config)
    );
    assert_eq!(model_ids(&merged), vec!["vendor/model".to_owned()]);

    let mixed = one(
        &format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {binary}\n      models_mode: merge\n      models: ['custom/model']\n"
        ),
        HarnessId::ClaudeCode,
    );
    assert_eq!(
        mixed.capabilities.models_source,
        Some(HarnessModelsSource::Merged)
    );
    assert!(model_ids(&mixed).contains(&"opus".to_owned()));
    assert!(model_ids(&mixed).contains(&"custom/model".to_owned()));
}

#[test]
fn illegal_model_tokens_fail_closed() {
    let file = tempfile::NamedTempFile::new().expect("binary");
    let binary = quote_path(file.path());
    for token in ["../secret", "~hidden", "foo..bar", ""] {
        let document = format!(
            "tasks:\n  harnesses:\n    codex:\n      binary: {binary}\n      models_mode: replace\n      models: ['{token}']\n"
        );
        offerings(&document).expect_err(token);
    }
    let accepted = one(
        &format!(
            "tasks:\n  harnesses:\n    codex:\n      binary: {binary}\n      models_mode: replace\n      models: ['foo~bar', 'notes.md']\n"
        ),
        HarnessId::Codex,
    );
    assert!(accepted.chat);
    assert_eq!(accepted.capabilities.effort_flag.as_deref(), Some("-c"));
    assert_eq!(
        model_ids(&accepted),
        vec!["foo~bar".to_owned(), "notes.md".to_owned()]
    );
}

#[test]
fn install_probe_rejects_missing_parents_and_directories() {
    let directory = tempfile::tempdir().expect("directory");
    let missing = directory.path().join("missing");
    assert_eq!(
        offerings(&format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {}\n",
            quote_path(&missing)
        ))
        .expect("offerings"),
        Vec::new()
    );

    let real = directory.path().join("real");
    std::fs::write(&real, b"bin").expect("file");
    let link = directory.path().join("linked");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    assert_eq!(
        offerings(&format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {}\n",
            quote_path(&link)
        ))
        .expect("offerings")
        .len(),
        1
    );
    let dangling = directory.path().join("dangling");
    std::os::unix::fs::symlink(directory.path().join("absent"), &dangling).expect("dangling");
    assert_eq!(
        offerings(&format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {}\n",
            quote_path(&dangling)
        ))
        .expect("offerings"),
        Vec::new()
    );

    std::fs::create_dir(directory.path().join("subdir")).expect("directory");
    assert_eq!(
        offerings(&format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {}\n",
            quote_path(&directory.path().join("subdir"))
        ))
        .expect("offerings"),
        Vec::new()
    );

    let nested = directory.path().join("bin");
    std::fs::create_dir(&nested).expect("bin");
    std::fs::write(nested.join("claude-agent-acp"), b"bin").expect("program");
    let adapter = AcpHarnessAdapter::new(&Config::default()).with_search_paths(vec![nested]);
    let listed = block_on(adapter.offerings()).expect("offerings");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed.first().expect("claude").harness_id,
        HarnessId::ClaudeCode
    );

    let parent = PathBuf::from("..").join("claude-agent-acp");
    assert_eq!(
        offerings(&format!(
            "tasks:\n  harnesses:\n    claude-code:\n      binary: {}\n",
            quote_path(&parent)
        ))
        .expect("offerings"),
        Vec::new()
    );
}

fn model_ids(offering: &HarnessOffering) -> Vec<String> {
    offering
        .capabilities
        .models
        .iter()
        .flatten()
        .map(|model| model.id.clone())
        .collect()
}
