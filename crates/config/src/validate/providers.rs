use crate::provider::{
    CliProvider, OpenaiProvider, ProviderBody, ProviderSlot, ProviderType, UnknownProvider,
};
use crate::report::ValidationResult;

use super::{hardcoded_key_warning, non_empty_entries, positive, required_string};

const OPENAI_COMPAT_SPLIT: &str = "The \"openai-compat\" provider was split into dedicated providers. Use \"vllm\" for a vLLM server, or \"llama-server\" for llama.cpp's llama-server — both take base_url.";

pub(super) fn validate(
    out: &mut ValidationResult,
    providers: Option<&std::collections::BTreeMap<String, ProviderSlot>>,
) {
    let Some(providers) = providers else {
        return;
    };
    if providers.is_empty() {
        out.error(
            "providers",
            "\"providers\" is empty — define at least one provider",
        );
        return;
    }
    for (name, slot) in providers {
        entry(out, name, slot.kind, &slot.body);
    }
}

fn entry(out: &mut ValidationResult, name: &str, kind: Option<ProviderType>, body: &ProviderBody) {
    let path = format!("providers.{name}");
    match kind {
        Some(kind) => match kind.removed_harness() {
            Some(harness) => out.error(
                format!("{path}.type"),
                format!(
                    "Provider \"{name}\" type \"{kind}\" is not built — reach {kind} models through the {harness} harness instead."
                ),
            ),
            None if kind == ProviderType::OpenaiCompat => out.error(&path, OPENAI_COMPAT_SPLIT),
            None => match body {
                ProviderBody::Openai(provider) => openai(out, name, kind, provider),
                ProviderBody::Cli(provider) => cli(out, name, kind, provider),
                ProviderBody::Unknown(_) | ProviderBody::Removed => {}
            },
        },
        None => {
            if let ProviderBody::Unknown(provider) = body {
                unknown(out, name, provider);
            }
        }
    }
}

fn openai(out: &mut ValidationResult, name: &str, kind: ProviderType, provider: &OpenaiProvider) {
    let path = format!("providers.{name}");
    required_string(
        out,
        &format!("{path}.model"),
        provider.model.as_deref(),
        &format!("Provider \"{name}\" is missing required field \"model\""),
    );
    positive(
        out,
        &format!("{path}.max_tokens"),
        provider.max_tokens,
        &format!("Provider \"{name}\" max_tokens must be a positive number"),
    );
    if kind.takes_base_url() && provider.base_url.as_deref().is_none_or(str::is_empty) {
        out.error(
            format!("{path}.base_url"),
            format!("Provider \"{name}\" requires \"base_url\""),
        );
    }
    if kind.takes_token_command() {
        positive(
            out,
            &format!("{path}.token_ttl_ms"),
            provider.token_ttl_ms,
            &format!("Provider \"{name}\" token_ttl_ms must be a positive number"),
        );
        positive(
            out,
            &format!("{path}.token_command_timeout_ms"),
            provider.token_command_timeout_ms,
            &format!("Provider \"{name}\" token_command_timeout_ms must be a positive number"),
        );
    }
    if kind.takes_catalog() {
        non_empty_entries(
            out,
            format!("{path}.models"),
            provider.models.as_ref(),
            format!(
                "Provider \"{name}\" models must be an array of non-empty strings (static catalog floor)"
            ),
        );
        positive(
            out,
            &format!("{path}.models_ttl_ms"),
            provider.models_ttl_ms,
            &format!("Provider \"{name}\" models_ttl_ms must be a positive number"),
        );
    }
    temperature(out, name, provider.temperature);
    if let Some(key) = &provider.api_key {
        hardcoded_key_warning(out, &format!("{path}.api_key"), name, key);
    }
    if provider.api_key.is_some() && provider.token_command.is_some() {
        out.warning(
            format!("{path}.token_command"),
            format!("Provider \"{name}\" has both api_key and token_command — token_command wins"),
        );
    }
}

fn cli(out: &mut ValidationResult, name: &str, kind: ProviderType, provider: &CliProvider) {
    let path = format!("providers.{name}");
    if kind != ProviderType::ClaudeCli {
        return;
    }
    if let Some(prompts) = &provider.permission_prompts
        && prompts != "ui"
        && prompts != "none"
    {
        out.error(
            format!("{path}.permission_prompts"),
            "permission_prompts must be \"ui\" or \"none\" (omit the key to leave spawns unchanged)",
        );
    }
    if let Some(timeout) = provider.permission_timeout_ms
        && (timeout == 0 || timeout > 600_000)
    {
        out.error(
            format!("{path}.permission_timeout_ms"),
            "permission_timeout_ms must be a positive integer of at most 600000 (default 60000)",
        );
    }
    non_empty_entries(
        out,
        format!("{path}.allowed_api_key_sources"),
        provider.allowed_api_key_sources.as_ref(),
        "allowed_api_key_sources must be an array of non-empty strings",
    );
}

fn unknown(out: &mut ValidationResult, name: &str, provider: &UnknownProvider) {
    let path = format!("providers.{name}");
    let shown = provider.declared.as_deref().unwrap_or(name);
    out.warning(
        &path,
        format!("Unknown provider type \"{shown}\" — make sure a registrar handles it"),
    );
    required_string(
        out,
        &format!("{path}.model"),
        provider.model.as_deref(),
        &format!("Provider \"{name}\" is missing required field \"model\""),
    );
    positive(
        out,
        &format!("{path}.max_tokens"),
        provider.max_tokens,
        &format!("Provider \"{name}\" max_tokens must be a positive number"),
    );
    temperature(out, name, provider.temperature);
    if let Some(key) = &provider.api_key {
        hardcoded_key_warning(out, &format!("{path}.api_key"), name, key);
    }
}

fn temperature(out: &mut ValidationResult, name: &str, value: Option<f64>) {
    let Some(value) = value else {
        return;
    };
    if (0.0..=2.0).contains(&value) {
        return;
    }
    out.warning(
        format!("providers.{name}.temperature"),
        format!("Provider \"{name}\" temperature {value} is outside typical range (0-2)"),
    );
}
