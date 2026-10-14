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

#[must_use]
pub fn replace_lone_surrogates(bytes: &[u8]) -> Vec<u8> {
    if !bytes.windows(2).any(|window| window == b"\\u") {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(byte) = bytes.get(index).copied() {
        if is_escape_at(bytes, index) {
            let code = optional_escape_code(bytes, index);
            let paired = code.is_some_and(|high| (0xD800..=0xDBFF).contains(&high))
                && is_escape_at(bytes, index + 6)
                && optional_escape_code(bytes, index + 6)
                    .is_some_and(|low| (0xDC00..=0xDFFF).contains(&low));
            if paired {
                if let Some(pair) = bytes.get(index..index + 12) {
                    out.extend_from_slice(pair);
                }
                index += 12;
            } else if code.is_some_and(|unit| (0xD800..=0xDFFF).contains(&unit)) {
                out.extend_from_slice(b"\\ufffd");
                index += 6;
            } else {
                out.push(byte);
                index += 1;
            }
        } else {
            out.push(byte);
            index += 1;
        }
    }
    out
}

fn is_escape_at(bytes: &[u8], index: usize) -> bool {
    bytes.get(index) == Some(&b'\\')
        && bytes.get(index + 1) == Some(&b'u')
        && preceded_by_even_backslashes(bytes, index)
        && bytes
            .get(index + 2..index + 6)
            .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
}

fn preceded_by_even_backslashes(bytes: &[u8], index: usize) -> bool {
    let mut count = 0;
    while let Some(previous) = index.checked_sub(count + 1).and_then(|at| bytes.get(at)) {
        if *previous != b'\\' {
            break;
        }
        count += 1;
    }
    count % 2 == 0
}

fn optional_escape_code(bytes: &[u8], index: usize) -> Option<u32> {
    let hex = bytes.get(index + 2..index + 6)?;
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS JSONBYTES: a non-hex escape has no surrogate code point"
        )
    )]
    let code = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok();
    code
}

#[cfg(test)]
mod surrogate_tests {
    use super::replace_lone_surrogates;

    #[test]
    fn escaped_backslashes_are_not_treated_as_escapes() {
        let sanitized = replace_lone_surrogates(br#"{"content":"a\\ud800"}"#);
        assert_eq!(&sanitized[..], br#"{"content":"a\\ud800"}"#);
        let sanitized = replace_lone_surrogates(br#""\ud83d\ude00""#);
        assert_eq!(&sanitized[..], br#""\ud83d\ude00""#);
        let sanitized = replace_lone_surrogates(br#""x\ud800y""#);
        assert_eq!(&sanitized[..], br#""x\ufffdy""#);
    }
}
