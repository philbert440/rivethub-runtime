use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;

use super::error::AgentError;

pub(crate) const NAME_MAX: usize = 128;
pub(crate) const MODEL_MAX: usize = 128;
pub(crate) const URL_MAX: usize = 512;
pub(crate) const DIRECTORY_MAX: usize = 512;
pub(crate) const EFFORT_MAX: usize = 64;
pub(crate) const SORT_ORDER_MAX: u32 = 1_000_000;
pub(crate) const UNNAMED: &str = "Unnamed Agent";
pub(crate) const DEFAULT_EFFORT: &str = "medium";

const NAME_TOO_LONG: &str = "name must be at most 128 characters";
const NAME_EMPTY: &str = "name must not be empty";
const COLOR: &str = "color must be a hex value";
const MODEL_TOO_LONG: &str = "model must be at most 128 characters";
const MODEL_CONTROL: &str = "model must not contain control characters";
const EFFORT: &str = "effort must be a 0-64 token";
const URL: &str = "nodeBaseUrl must be at most 512 characters";
const SORT: &str = "sortOrder must be an integer from 0 to 1000000";
const DIRECTORY_ABS: &str = "directory must be an absolute path";
const DIRECTORY_ESCAPE: &str = "directory escapes its requested path";
const DIRECTORY_LONG: &str = "directory must be at most 512 bytes";
const DIRECTORY_NUL: &str = "directory must not contain NUL";
const NODE_IMMUTABLE: &str = "node is immutable; recreate the agent";

pub(crate) fn name_create(name: &str) -> Result<String, AgentError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(UNNAMED.to_owned());
    }
    bounded(name, NAME_MAX, NAME_TOO_LONG)?;
    Ok(name.to_owned())
}

pub(crate) fn name_update(name: &str) -> Result<String, AgentError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AgentError::BadRequest(NAME_EMPTY));
    }
    bounded(name, NAME_MAX, NAME_TOO_LONG)?;
    Ok(name.to_owned())
}

pub(crate) fn color(value: &str) -> Result<(), AgentError> {
    if value.is_empty() || is_hex_color(value) {
        Ok(())
    } else {
        Err(AgentError::BadRequest(COLOR))
    }
}

pub(crate) fn model(value: &str) -> Result<(), AgentError> {
    if value.chars().count() > MODEL_MAX {
        return Err(AgentError::BadRequest(MODEL_TOO_LONG));
    }
    if value.chars().any(char::is_control) {
        return Err(AgentError::BadRequest(MODEL_CONTROL));
    }
    Ok(())
}

pub(crate) fn effort(value: &str) -> Result<(), AgentError> {
    let token = value.len() <= EFFORT_MAX && value.chars().all(effort_byte);
    if token {
        Ok(())
    } else {
        Err(AgentError::BadRequest(EFFORT))
    }
}

pub(crate) enum CatalogHarness {
    Known(HarnessId),
    Unknown(String),
}

pub(crate) fn catalog_harness(model: &str) -> Option<CatalogHarness> {
    if model.is_empty() {
        return None;
    }
    Some(match model {
        "claude" => CatalogHarness::Known(HarnessId::ClaudeCode),
        "grok" | "grok-fast" => CatalogHarness::Known(HarnessId::GrokBuild),
        "kimi" => CatalogHarness::Known(HarnessId::KimiCode),
        "hermes" => CatalogHarness::Known(HarnessId::Hermes),
        other => parse_catalog(other),
    })
}

fn parse_catalog(model: &str) -> CatalogHarness {
    if let Ok(id) = HarnessId::from_str(model) {
        CatalogHarness::Known(id)
    } else {
        CatalogHarness::Unknown(model.to_owned())
    }
}

pub(crate) fn node_base_url(value: &str) -> Result<(), AgentError> {
    bounded(value, URL_MAX, URL)
}

pub(crate) fn sort_order(value: u32) -> Result<(), AgentError> {
    if value <= SORT_ORDER_MAX {
        Ok(())
    } else {
        Err(AgentError::BadRequest(SORT))
    }
}

pub(crate) fn host_node(requested: Option<&str>, here: &str) -> Result<String, AgentError> {
    match requested {
        None => Ok(here.to_owned()),
        Some(node) if node == here => Ok(here.to_owned()),
        Some(_) => Err(AgentError::BadRequestOwned(format!(
            "agent must be created on its hosting node ({here})"
        ))),
    }
}

pub(crate) fn node_change(current: Option<&str>, requested: &str) -> Result<(), AgentError> {
    if current == Some(requested) {
        Ok(())
    } else {
        Err(AgentError::BadRequest(NODE_IMMUTABLE))
    }
}

pub(crate) fn allow(list: Option<&[HarnessId]>, id: HarnessId) -> Result<(), AgentError> {
    match list {
        None => Ok(()),
        Some(entries) if entries.contains(&id) => Ok(()),
        Some(_) => Err(AgentError::HarnessDenied { harness: id }),
    }
}

pub(crate) fn name_free(
    agents: &[AgentPreset],
    name: &str,
    except: Option<&str>,
) -> Result<(), AgentError> {
    let key = name_key(name);
    let taken = agents
        .iter()
        .any(|agent| except != Some(agent.id.as_str()) && name_key(&agent.name) == key);
    if taken {
        Err(AgentError::conflict(format!(
            "an agent named \"{name}\" already exists"
        )))
    } else {
        Ok(())
    }
}

pub(crate) fn foreign<'a>(agent: &'a AgentPreset, here: &str) -> Option<&'a str> {
    agent.node.as_deref().filter(|node| *node != here)
}

pub(crate) fn hosted(name: &str, node: &str) -> AgentError {
    AgentError::Hosted {
        name: name.to_owned(),
        node: node.to_owned(),
    }
}

pub(crate) fn contained(root: &Path, child: &Path) -> Result<bool, AgentError> {
    let root = normalize(root)?;
    let child = normalize(child)?;
    Ok(child.starts_with(root))
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub(crate) fn directory_str(raw: &str) -> Result<PathBuf, AgentError> {
    if raw.len() > DIRECTORY_MAX {
        return Err(AgentError::BadRequest(DIRECTORY_LONG));
    }
    if raw.as_bytes().contains(&0) {
        return Err(AgentError::BadRequest(DIRECTORY_NUL));
    }
    directory_path(Path::new(raw))
}

pub(crate) fn directory_path(path: &Path) -> Result<PathBuf, AgentError> {
    if !path.is_absolute() {
        return Err(AgentError::BadRequest(DIRECTORY_ABS));
    }
    normalize(path)
}

fn normalize(path: &Path) -> Result<PathBuf, AgentError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(component),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(AgentError::BadRequest(DIRECTORY_ESCAPE));
            }
        }
    }
    if normalized.parent().is_none() {
        return Err(AgentError::BadRequest(DIRECTORY_ESCAPE));
    }
    Ok(normalized)
}

fn bounded(value: &str, max: usize, message: &'static str) -> Result<(), AgentError> {
    if value.chars().count() > max {
        Err(AgentError::BadRequest(message))
    } else {
        Ok(())
    }
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn effort_byte(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | '[' | ']' | ':' | '-')
}

fn is_hex_color(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 6) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::contained;

    #[test]
    fn containment_is_component_wise() {
        let root = Path::new("/tmp/agents");
        assert!(matches!(
            contained(root, Path::new("/tmp/agents/p-1")),
            Ok(true)
        ));
        assert!(matches!(
            contained(root, Path::new("/tmp/agents/./quoted name")),
            Ok(true)
        ));
        assert!(matches!(
            contained(root, Path::new("/tmp/agents/ordinary$substitution")),
            Ok(true)
        ));
        assert!(matches!(
            contained(root, Path::new("/tmp/agents-evil")),
            Ok(false)
        ));
        assert!(contained(root, Path::new("/tmp/agents/../agents-evil")).is_err());
        assert!(contained(root, Path::new("/tmp/agents/../../etc")).is_err());
        let root_text = root.display().to_string();
        let sibling_text = format!("{root_text}-evil");
        assert!(sibling_text.starts_with(&root_text));
        assert!(matches!(
            contained(root, Path::new(&sibling_text)),
            Ok(false)
        ));
    }
}
