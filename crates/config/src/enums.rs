pub use wire::harness::HarnessId;
pub use wire::stream::ThinkingLevel;
pub use wire::task::Effort;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Deserialize,
    strum::Display,
    strum::EnumString,
    strum::EnumIter,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DeploymentTarget {
    Docker,
    Proxmox,
    Kubernetes,
    Manual,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Isolation {
    Inherit,
    Isolated,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ModelsMode {
    Discover,
    Replace,
    Merge,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum VerifierExecutor {
    ChatLoop,
    HarnessSession,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum EmbedWireShape {
    Openai,
    Native,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum MemoryBackend {
    Postgres,
    Sqlite,
}

pub(crate) fn listed<T: std::fmt::Display + strum::IntoEnumIterator>() -> String {
    T::iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
