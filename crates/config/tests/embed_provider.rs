use config::{Config, ConfigError, Environment, MemoryBackend};

fn memory_yaml(backend: MemoryBackend, keys: &str) -> String {
    let location = match backend {
        MemoryBackend::Sqlite => "    path: ':memory:'\n",
        MemoryBackend::Postgres => "    connection_string: postgres://example.com/db\n",
    };
    format!(
        "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: main-provider\nproviders:\n  main-provider:\n    type: claude-cli\n    model: acme-model\n  embedder:\n    type: vllm\n    base_url: http://embed.example.com/v1/\n    model: served-model\n    token_command:\n      - /home/user/bin/token\nmemory:\n  backend: {backend}\n  {backend}:\n{location}    embed_model: embedding-model\n{keys}"
    )
}

fn backends() -> [MemoryBackend; 2] {
    [MemoryBackend::Postgres, MemoryBackend::Sqlite]
}

fn rejected(backend: MemoryBackend, keys: &str) -> Result<String, String> {
    match Config::from_str(&Environment::default(), &memory_yaml(backend, keys)) {
        Err(ConfigError::Validation { result }) => Ok(result.to_string()),
        Err(other) => Err(format!("{backend} {keys}: {other}")),
        Ok(_) => Err(format!("{backend} {keys}: expected a validation error")),
    }
}

#[test]
fn embed_provider_names_the_provider_that_serves_embeddings()
-> Result<(), Box<dyn std::error::Error>> {
    for backend in backends() {
        let env = Environment::default();
        let loaded = Config::from_str(
            &env,
            &memory_yaml(backend, "    embed_provider: embedder\n"),
        )?;
        let provider = loaded
            .config
            .embed_provider(backend)
            .ok_or("the embed provider")?;
        assert_eq!(
            provider.token_command,
            Some(vec!["/home/user/bin/token".to_owned()]),
            "{backend}"
        );
        let endpoint = match backend {
            MemoryBackend::Postgres => loaded.config.memory_embed_endpoint(&env),
            MemoryBackend::Sqlite => loaded.config.sqlite_embed_endpoint(&env),
        };
        assert_eq!(
            endpoint.as_deref(),
            Some("http://embed.example.com/v1/"),
            "{backend}"
        );
        assert!(
            loaded
                .warnings
                .iter()
                .all(|finding| !finding.path.contains("embed_provider")),
            "{backend}: {:?}",
            loaded.warnings
        );
    }
    Ok(())
}

#[test]
fn embed_provider_with_a_static_endpoint_or_key_is_rejected() -> Result<(), String> {
    for backend in backends() {
        let endpoint = rejected(
            backend,
            "    embed_provider: embedder\n    embed_endpoint: http://example.com/v1\n",
        )?;
        assert!(
            endpoint.contains(&format!("memory.{backend}.embed_provider")),
            "{endpoint}"
        );
        assert!(
            endpoint.contains("\"embed_provider\" and \"embed_endpoint\" cannot both be set"),
            "{endpoint}"
        );
        let key = rejected(
            backend,
            "    embed_provider: embedder\n    embed_api_key: ${EMBED_KEY}\n",
        )?;
        assert!(
            key.contains("\"embed_provider\" and \"embed_api_key\" cannot both be set"),
            "{key}"
        );
    }
    Ok(())
}

#[test]
fn an_unknown_embed_provider_is_rejected_naming_the_key() -> Result<(), String> {
    for backend in backends() {
        let text = rejected(backend, "    embed_provider: missing\n")?;
        assert!(
            text.contains(&format!(
                "Provider \"missing\" referenced by memory.{backend}.embed_provider is not defined in [providers]. Available: embedder, main-provider"
            )),
            "{text}"
        );
    }
    Ok(())
}

#[test]
fn a_cli_embed_provider_is_rejected() -> Result<(), String> {
    for backend in backends() {
        let text = rejected(backend, "    embed_provider: main-provider\n")?;
        assert!(
            text.contains(&format!(
                "Provider \"main-provider\" referenced by memory.{backend}.embed_provider cannot serve embeddings — use an openai-compatible or vllm provider"
            )),
            "{text}"
        );
    }
    Ok(())
}
