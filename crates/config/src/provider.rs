use std::path::PathBuf;

use serde::Deserialize;

use crate::enums::Effort;
use crate::free_form::FreeForm;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ProviderType {
    OpenaiCompatible,
    Vllm,
    LlamaServer,
    Ollama,
    Cli,
    ClaudeCli,
    CodexCli,
    GrokCli,
    HermesCli,
    KimiCode,
    OpencodeCli,
    PiCli,
    QwenCode,
    Anthropic,
    Google,
    Gemini,
    Xai,
    OpenaiCompat,
}

impl ProviderType {
    pub fn is_removed(self) -> bool {
        self.removed_harness().is_some()
    }

    pub fn removed_harness(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("claude-code"),
            Self::Google | Self::Gemini => Some("gemini"),
            Self::Xai => Some("grok-build"),
            Self::OpenaiCompatible
            | Self::Vllm
            | Self::LlamaServer
            | Self::Ollama
            | Self::Cli
            | Self::ClaudeCli
            | Self::CodexCli
            | Self::GrokCli
            | Self::HermesCli
            | Self::KimiCode
            | Self::OpencodeCli
            | Self::PiCli
            | Self::QwenCode
            | Self::OpenaiCompat => None,
        }
    }

    pub fn is_openai_family(self) -> bool {
        matches!(
            self,
            Self::OpenaiCompatible | Self::Vllm | Self::LlamaServer | Self::Ollama
        )
    }

    pub fn is_cli_family(self) -> bool {
        matches!(
            self,
            Self::Cli
                | Self::ClaudeCli
                | Self::CodexCli
                | Self::GrokCli
                | Self::HermesCli
                | Self::KimiCode
                | Self::OpencodeCli
                | Self::PiCli
                | Self::QwenCode
        )
    }

    pub fn takes_base_url(self) -> bool {
        self.is_openai_family()
    }

    pub fn takes_token_command(self) -> bool {
        matches!(
            self,
            Self::OpenaiCompatible | Self::Vllm | Self::LlamaServer
        )
    }

    pub fn takes_catalog(self) -> bool {
        self.takes_token_command()
    }
}

#[derive(Debug)]
pub struct ProviderSlot {
    pub kind: Option<ProviderType>,
    pub body: ProviderBody,
}

#[derive(Debug)]
pub enum ProviderBody {
    Openai(OpenaiProvider),
    Cli(CliProvider),
    Unknown(UnknownProvider),
    Removed,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct OpenaiProvider {
    pub model: Option<String>,
    pub name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    #[serde(rename = "type")]
    pub declared: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub token_command: Option<Vec<String>>,
    pub token_ttl_ms: Option<u64>,
    pub token_command_timeout_ms: Option<u64>,
    pub models: Option<Vec<String>>,
    pub models_ttl_ms: Option<u64>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i32>,
    pub min_p: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub seed: Option<i64>,
    pub default_tool_choice: Option<String>,
    pub verify_model_on_init: Option<bool>,
    pub repetition_penalty: Option<f64>,
    pub min_tokens: Option<u32>,
    pub stop: Option<Vec<String>>,
    pub mm_processor_kwargs: Option<FreeForm>,
    pub chat_template_kwargs: Option<FreeForm>,
    pub extra_body: Option<FreeForm>,
    pub api_prefix: Option<String>,
    pub models_url: Option<String>,
    pub probe_models: Option<bool>,
    pub num_ctx: Option<u32>,
    pub keep_alive: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct CliProvider {
    pub model: Option<String>,
    pub name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    #[serde(rename = "type")]
    pub declared: Option<String>,
    pub binary: Option<String>,
    pub tools: Option<String>,
    pub effort: Option<Effort>,
    pub permission_mode: Option<String>,
    pub exclude_dynamic_sections: Option<bool>,
    pub append_system_prompt: Option<bool>,
    pub cwd: Option<PathBuf>,
    pub timeout_ms: Option<u64>,
    pub permission_prompts: Option<String>,
    pub permission_timeout_ms: Option<u64>,
    pub allowed_api_key_sources: Option<Vec<String>>,
    pub home: Option<PathBuf>,
    pub session: Option<String>,
    pub profile: Option<String>,
    pub sandbox: Option<String>,
    pub approve_for_me: Option<bool>,
    pub skip_git_repo_check: Option<bool>,
    pub reasoning_effort: Option<Effort>,
    pub max_turns: Option<u32>,
    pub no_plan: Option<bool>,
    pub system_prompt: Option<String>,
    pub allow: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct UnknownProvider {
    #[serde(rename = "type")]
    pub declared: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub api_key: Option<String>,
}
