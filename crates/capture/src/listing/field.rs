use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub(crate) struct Field(Box<RawValue>);

impl Field {
    pub(crate) fn text(&self) -> Option<String> {
        self.is_string().then(|| typed(self.0.get())).flatten()
    }

    pub(crate) fn number(&self) -> Option<f64> {
        let raw = self.0.get();
        raw.starts_with(|first: char| first == '-' || first.is_ascii_digit())
            .then(|| typed(raw))
            .flatten()
    }

    pub(crate) fn count(&self) -> Option<u64> {
        self.0
            .get()
            .starts_with(|first: char| first.is_ascii_digit())
            .then(|| typed(self.0.get()))
            .flatten()
    }

    pub(crate) fn flag(&self) -> bool {
        self.0.get() == "true"
    }

    pub(crate) fn raw(&self) -> &str {
        self.0.get()
    }

    pub(crate) fn items(&self) -> Option<Vec<Self>> {
        self.0
            .get()
            .starts_with('[')
            .then(|| typed(self.0.get()))
            .flatten()
    }

    pub(crate) fn object<T: DeserializeOwned>(&self) -> Option<T> {
        self.is_object().then(|| typed(self.0.get())).flatten()
    }

    pub(crate) fn is_object(&self) -> bool {
        self.0.get().starts_with('{')
    }

    pub(crate) fn is_string(&self) -> bool {
        self.0.get().starts_with('"')
    }

    pub(crate) fn is_text(&self, expected: &str) -> bool {
        self.text().is_some_and(|text| text == expected)
    }
}

pub(crate) fn text_of(field: Option<&Field>) -> Option<String> {
    field.and_then(Field::text)
}

pub(crate) fn first_present<'a>(fields: &[Option<&'a Field>]) -> Option<&'a Field> {
    fields.iter().find_map(|field| *field)
}

pub(crate) fn typed<T: DeserializeOwned>(json: &str) -> Option<T> {
    match serde_json::from_str(json) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::trace!(%error, "store value does not have the expected shape and is skipped");
            None
        }
    }
}

pub(crate) fn object_line<T: DeserializeOwned>(line: &str) -> Option<T> {
    let trimmed = line.trim();
    if trimmed.starts_with('{') {
        typed(trimmed)
    } else {
        None
    }
}
