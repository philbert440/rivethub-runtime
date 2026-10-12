use config::{type_document, validate_document};

fn findings(document: &serde_yaml_ng::Value) -> Vec<String> {
    let typed = type_document(document);
    if let Ok(typed) = &typed {
        assert!(!typed.ignored.is_empty() || document.get("providers").is_none());
    }
    let result = validate_document(document);
    let mut lines: Vec<String> = result
        .errors
        .iter()
        .chain(&result.warnings)
        .map(|finding| format!("{} | {}", finding.path, finding.message))
        .collect();
    lines.sort();
    lines
}

fn parsed(yaml: &str) -> Result<serde_yaml_ng::Value, serde_yaml_ng::Error> {
    serde_yaml_ng::from_str(yaml)
}

#[test]
fn ignored_paths_reach_the_chooser() -> Result<(), Box<dyn std::error::Error>> {
    let lines = findings(&parsed(
        "runtime:\n  workspace: /home/user/work\n  tagger_allow_protected_removals: 5\nagents:\n  main:\n    provider: p\n    model: m\nproviders:\n  p:\n    model: m\n",
    )?);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Unknown runtime key \"tagger_allow_protected_removals\"")),
        "runtime unknown key missing: {lines:?}"
    );

    let lines = findings(&parsed(
        "runtime:\n  workspace: w\n  default_agent: main\n  heartbeats:\n    - agent: main\n      schedule: 30m\n      prompt: hi\n      bogus: 1\nagents:\n  main:\n    provider: p\n    tools:\n      exclude: []\n      bogus: 2\nproviders:\n  p:\n    model: m\n    bogus: 3\nden:\n  port: 3000\n  bogus: 4\nmemory:\n  sqlite:\n    path: ':memory:'\n    bogus: 5\n  capture:\n    redaction:\n      enabled: false\n      bogus: 6\n  postgres:\n    connection_string: postgres://example.com/db\n    embedded:\n      port: 5433\n      bogus: 7\ntasks:\n  harnesses:\n    claude-code:\n      binary: claude\n      bogus: 8\n  eval:\n    enabled: true\n    bogus: 9\nchannels:\n  agent:\n    port: 3100\n    bogus: 10\ndeployment:\n  target: docker\n  bogus: 11\nmesh:\n  enabled: false\n  bogus: 12\ntransports:\n  mcp:\n    port: 5700\n    bogus: 13\n",
    )?);
    for needle in [
        "runtime.heartbeats[0].bogus | Unknown heartbeat key \"bogus\"",
        "agents.main.tools.bogus | Unknown tools filter key \"bogus\" — expected \"exclude\" or \"include\"",
        "den.bogus | Unknown den key \"bogus\"",
        "memory.sqlite.bogus | Unknown memory.sqlite key \"bogus\"",
        "memory.capture.redaction.bogus | Unknown memory.capture.redaction key \"bogus\"",
        "memory.postgres.embedded.bogus | Unknown memory.postgres.embedded key \"bogus\"",
        "tasks.harnesses.claude-code.bogus | Unknown key \"bogus\"",
        "tasks.eval.bogus | Unknown tasks.eval key \"bogus\"",
        "channels.agent.bogus | Unknown key \"bogus\" for channel type \"agent\"",
        "deployment.bogus | Unknown deployment key \"bogus\"",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(needle)),
            "missing {needle} in {lines:?}"
        );
    }
    assert!(
        !lines.iter().any(|line| line.contains("mesh.bogus")),
        "mesh has no unknown-key check: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("providers.p.bogus")),
        "unknown provider ids do not warn per key: {lines:?}"
    );

    let lines = findings(&parsed(
        "runtime:\n  workspace: w\n  default_agent: main\nagents:\n  main:\n    provider: acme\nproviders:\n  vllm:\n    model: m\n    base_url: http://example.com/v1\n    bogus: 1\n",
    )?);
    assert!(
        lines.iter().any(|line| line
            .contains("providers.vllm.bogus | Unknown key \"bogus\" for provider type \"vllm\"")),
        "kept provider ids warn per key: {lines:?}"
    );
    Ok(())
}
