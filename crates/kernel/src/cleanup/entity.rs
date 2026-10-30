use serde::Deserialize;
use serde::Serialize;
use strum::IntoEnumIterator as _;

use crate::topics::fold;

const PATH_SEPARATOR: &str = " › ";

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumIter,
)]
#[serde(rename_all = "kebab-case")]
pub enum EntityType {
    People,
    Agents,
    Hosts,
    Hardware,
    Models,
    Services,
    Software,
    #[strum(to_string = "CLIs and tools")]
    ClisAndTools,
    Apps,
    #[strum(to_string = "Libraries and frameworks")]
    LibrariesAndFrameworks,
    #[strum(to_string = "OS and platforms")]
    OsAndPlatforms,
    #[strum(to_string = "Plugins and skills")]
    PluginsAndSkills,
    Repos,
    Domains,
    Databases,
}

impl EntityType {
    #[must_use]
    pub const fn parent(self) -> Option<Self> {
        match self {
            Self::ClisAndTools
            | Self::Apps
            | Self::LibrariesAndFrameworks
            | Self::OsAndPlatforms
            | Self::PluginsAndSkills => Some(Self::Software),
            Self::People
            | Self::Agents
            | Self::Hosts
            | Self::Hardware
            | Self::Models
            | Self::Services
            | Self::Software
            | Self::Repos
            | Self::Domains
            | Self::Databases => None,
        }
    }

    #[must_use]
    pub fn path(self) -> String {
        match self.parent() {
            Some(parent) => format!("{parent}{PATH_SEPARATOR}{self}"),
            None => self.to_string(),
        }
    }

    #[must_use]
    pub fn named(text: &str) -> Option<Self> {
        let leaf = text.rsplit(PATH_SEPARATOR.trim()).next()?.trim();
        Self::iter().find(|kind| kind.to_string().eq_ignore_ascii_case(leaf))
    }

    #[must_use]
    pub fn of_reference(reference: &str) -> Option<(Self, String)> {
        let (prefix, name) = reference.split_once(':')?;
        let prefix = prefix.trim();
        ReferenceKind::iter()
            .find(|kind| kind.to_string().eq_ignore_ascii_case(prefix))
            .map(|kind| (kind.entity_type(), fold(name)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumIter)]
#[strum(serialize_all = "kebab-case")]
enum ReferenceKind {
    Person,
    People,
    User,
    Agent,
    Bot,
    Host,
    Container,
    Vm,
    Hardware,
    Gpu,
    Device,
    Model,
    Service,
    Software,
    Tool,
    Cli,
    App,
    Library,
    Framework,
    Package,
    Os,
    Platform,
    Plugin,
    Skill,
    Repo,
    Domain,
    Db,
    Database,
}

impl ReferenceKind {
    const fn entity_type(self) -> EntityType {
        match self {
            Self::Person | Self::People | Self::User => EntityType::People,
            Self::Agent | Self::Bot => EntityType::Agents,
            Self::Host | Self::Container | Self::Vm => EntityType::Hosts,
            Self::Hardware | Self::Gpu | Self::Device => EntityType::Hardware,
            Self::Model => EntityType::Models,
            Self::Service => EntityType::Services,
            Self::Software => EntityType::Software,
            Self::Tool | Self::Cli => EntityType::ClisAndTools,
            Self::App => EntityType::Apps,
            Self::Library | Self::Framework | Self::Package => EntityType::LibrariesAndFrameworks,
            Self::Os | Self::Platform => EntityType::OsAndPlatforms,
            Self::Plugin | Self::Skill => EntityType::PluginsAndSkills,
            Self::Repo => EntityType::Repos,
            Self::Domain => EntityType::Domains,
            Self::Db | Self::Database => EntityType::Databases,
        }
    }
}
