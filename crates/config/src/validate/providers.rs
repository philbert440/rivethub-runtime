use crate::provider::{
    CliProvider, OpenaiProvider, ProviderBody, ProviderSlot, ProviderType, UnknownProvider,
};
use crate::report::ValidationResult;

use super::{hardcoded_key_warning, non_empty_entries, positive, required_string};

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
        entry(out, name, slot);
    }
}

fn entry(out: &mut ValidationResult, name: &str, slot: &ProviderSlot) {
    let path = format!("providers.{name}");
    match &slot.body {
        ProviderBody::Removed(removed) => out.error(
            format!("{path}.type"),
            format!(
                "Provider \"{name}\" type \"{removed}\" is not built — {}.",
                removed.message()
            ),
        ),
        ProviderBody::Openai(provider) => openai(out, name, provider),
        ProviderBody::Cli(provider) => cli(out, name, slot.kind, provider),
        ProviderBody::Unknown(provider) => unknown(out, name, provider),
    }
}

fn openai(out: &mut ValidationResult, name: &str, provider: &OpenaiProvider) {
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
    if provider.base_url.as_deref().is_none_or(str::is_empty) {
        out.error(
            format!("{path}.base_url"),
            format!("Provider \"{name}\" requires \"base_url\""),
        );
    }
    if let Some(key) = &provider.api_key {
        hardcoded_key_warning(out, &format!("{path}.api_key"), name, key);
    }
}

fn cli(out: &mut ValidationResult, name: &str, kind: Option<ProviderType>, provider: &CliProvider) {
    let path = format!("providers.{name}");
    if kind != Some(ProviderType::ClaudeCli) {
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
    if let Some(key) = &provider.api_key {
        hardcoded_key_warning(out, &format!("{path}.api_key"), name, key);
    }
}
