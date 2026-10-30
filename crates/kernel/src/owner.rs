#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Owner(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid owner id {0:?}")]
pub struct InvalidOwner(pub String);

impl Owner {
    pub fn new(id: String) -> Result<Self, InvalidOwner> {
        if id.is_empty() || id.chars().any(char::is_control) {
            return Err(InvalidOwner(id));
        }
        Ok(Self(id))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
