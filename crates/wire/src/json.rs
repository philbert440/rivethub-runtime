use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Extra(Map<String, Value>);

impl Extra {
    pub fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    #[must_use]
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonObject(Map<String, Value>);

impl JsonObject {
    pub fn from_serialize<T: Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        serde_json::from_value(serde_json::to_value(value)?)
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    #[must_use]
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    pub fn insert<T: Serialize>(&mut self, key: &str, value: T) -> Result<(), serde_json::Error> {
        self.0.insert(key.to_owned(), serde_json::to_value(value)?);
        Ok(())
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.0.retain(|key, _| keep(key));
    }

    pub fn visit_strings_mut(&mut self, visit: &mut impl FnMut(&str, &mut String)) {
        for (key, value) in &mut self.0 {
            visit_strings(key, value, visit);
        }
    }
}

fn visit_strings(key: &str, value: &mut Value, visit: &mut impl FnMut(&str, &mut String)) {
    match value {
        Value::String(text) => visit(key, text),
        Value::Array(items) => {
            for item in items {
                visit_strings(key, item, visit);
            }
        }
        Value::Object(members) => {
            for (name, member) in members {
                visit_strings(name, member, visit);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Sample {
        name: &'static str,
        count: u32,
    }

    #[test]
    fn objects_are_built_and_edited_through_serde() {
        let mut object = JsonObject::from_serialize(&Sample {
            name: "a",
            count: 2,
        })
        .unwrap();
        object.insert("flag", true).unwrap();
        object.retain(|key| key != "count");
        assert_eq!(
            serde_json::to_string(&object).unwrap(),
            r#"{"flag":true,"name":"a"}"#
        );
        assert!(JsonObject::from_serialize(&[1, 2]).is_err());
    }

    #[test]
    fn string_leaves_are_visited_with_their_member_name() {
        let mut object: JsonObject =
            serde_json::from_str(r#"{"a":"x","list":["y",{"b":"z"}],"n":1,"none":null}"#).unwrap();
        let mut seen = Vec::new();
        object.visit_strings_mut(&mut |key, text| {
            seen.push(format!("{key}={text}"));
            text.push('!');
        });
        assert_eq!(seen, ["a=x", "list=y", "b=z"]);
        assert_eq!(
            serde_json::to_string(&object).unwrap(),
            r#"{"a":"x!","list":["y!",{"b":"z!"}],"n":1,"none":null}"#
        );
    }
}
