use std::fmt::Formatter;

use serde::Deserialize;
use serde::Serialize;
use serde::de::Deserializer;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;
use thiserror::Error;
use wire::JsonObject;

#[derive(Debug, Clone, PartialEq)]
pub enum JsJson {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

#[derive(Debug, Error)]
pub enum JsonError {
    #[error("capture JSON encoding failed: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("capture JSON decode failed: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("capture expected a JSON object")]
    NotAnObject,
}

pub fn parse(text: &str) -> Result<JsJson, JsonError> {
    serde_json::from_str(text).map_err(JsonError::Decode)
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        customserde,
        reason = "CONTRACT 2 exception: JsJson is capture's one order-keeping JSON model, so the event id can hash the harness's member order"
    )
)]
impl<'de> Deserialize<'de> for JsJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsJsonVisitor)
    }
}

pub fn members_of(object: &JsonObject) -> Result<Vec<(String, JsJson)>, JsonError> {
    let text = serde_json::to_string(object).map_err(JsonError::Encode)?;
    match parse(&text)? {
        JsJson::Object(members) => Ok(members),
        JsJson::Nil
        | JsJson::Bool(_)
        | JsJson::Number(_)
        | JsJson::String(_)
        | JsJson::Array(_) => Err(JsonError::NotAnObject),
    }
}

pub fn to_object(value: &JsJson) -> Result<JsonObject, JsonError> {
    serde_json::from_str(&spelling(value)).map_err(JsonError::Decode)
}

pub fn object_of(members: Vec<(String, JsJson)>) -> Result<JsonObject, JsonError> {
    to_object(&JsJson::Object(members))
}

pub fn set_member(members: &mut Vec<(String, JsJson)>, key: &str, value: JsJson) {
    if let Some(slot) = members.iter_mut().find(|(name, _)| name == key) {
        slot.1 = value;
    } else {
        members.push((key.to_owned(), value));
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "member counts and byte counts are integers far below f64 precision"
)]
#[must_use]
pub fn number(value: usize) -> JsJson {
    JsJson::Number(value as f64)
}

pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<JsonObject, JsonError> {
    let text = serde_json::to_string(value).map_err(JsonError::Encode)?;
    serde_json::from_str(&text).map_err(JsonError::Decode)
}

pub fn decode<T: serde::de::DeserializeOwned>(object: &JsonObject) -> Result<T, JsonError> {
    let text = serde_json::to_string(object).map_err(JsonError::Encode)?;
    serde_json::from_str(&text).map_err(JsonError::Decode)
}

pub fn object_bytes(object: &JsonObject) -> Result<usize, JsonError> {
    Ok(serde_json::to_string(object)
        .map_err(JsonError::Encode)?
        .len())
}

pub fn object_chars(object: &JsonObject) -> Result<usize, JsonError> {
    Ok(serde_json::to_string(object)
        .map_err(JsonError::Encode)?
        .chars()
        .count())
}

struct JsJsonVisitor;

impl<'de> Visitor<'de> for JsJsonVisitor {
    type Value = JsJson;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(JsJson::Bool(value))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "JS numbers are f64; the recorded hash spells the f64, exactly like the harness"
    )]
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(JsJson::Number(value as f64))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "JS numbers are f64; the recorded hash spells the f64, exactly like the harness"
    )]
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(JsJson::Number(value as f64))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(JsJson::Number(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(JsJson::String(value.to_owned()))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(JsJson::Nil)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = access.next_element::<Self::Value>()? {
            items.push(item);
        }
        Ok(JsJson::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut members: Vec<(String, JsJson)> = Vec::new();
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value::<JsJson>()?;
            match members.iter_mut().find(|(name, _)| *name == key) {
                Some(slot) => slot.1 = value,
                None => members.push((key, value)),
            }
        }
        Ok(JsJson::Object(members))
    }
}

#[must_use]
pub fn spelling(value: &JsJson) -> String {
    match value {
        JsJson::Nil => "null".to_owned(),
        JsJson::Bool(flag) => {
            if *flag {
                "true".to_owned()
            } else {
                "false".to_owned()
            }
        }
        JsJson::Number(number) => spell_number(*number),
        JsJson::String(text) => quote(text),
        JsJson::Array(items) => {
            let mut out = String::new();
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&spelling(item));
            }
            out.push(']');
            out
        }
        JsJson::Object(members) => {
            let mut ordered: Vec<&(String, JsJson)> = members.iter().collect();
            ordered.sort_by_key(|(key, _)| (array_index(key).is_none(), array_index(key)));
            let mut out = String::new();
            out.push('{');
            for (index, (key, member)) in ordered.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&quote(key));
                out.push(':');
                out.push_str(&spelling(member));
            }
            out.push('}');
            out
        }
    }
}

#[expect(
    clippy::expect_used,
    reason = "serde_json serialization of a &str is a fixed string-escape pass and cannot fail"
)]
fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("JSON string escaping of a &str cannot fail")
}

fn array_index(key: &str) -> Option<u32> {
    if key == "0" {
        return Some(0);
    }
    if key.starts_with('0') {
        return None;
    }
    key.parse::<u32>().ok()
}

#[expect(
    clippy::expect_used,
    reason = "the LowerExp form of a finite f64 always carries an exponent whose magnitude fits i32"
)]
fn spell_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("the LowerExp form of a finite f64 always carries an exponent");
    let exponent: i32 = exponent
        .parse()
        .expect("f64 exponents span -324..=308 and fit i32");
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if (-6..21).contains(&exponent) {
        positional(&digits, negative, exponent)
    } else {
        let sign = if exponent < 0 { "-" } else { "+" };
        let prefix = if negative { "-" } else { "" };
        format!("{prefix}{mantissa}e{sign}{}", exponent.abs())
    }
}

#[expect(
    clippy::cast_sign_loss,
    reason = "positional only sees exponents in -6..21; the non-negative branch stays non-negative"
)]
fn positional(digits: &str, negative: bool, exponent: i32) -> String {
    let sign = if negative { "-" } else { "" };
    if exponent < 0 {
        let padding = (exponent.unsigned_abs() as usize) - 1;
        return format!("{sign}0.{}{digits}", "0".repeat(padding));
    }
    let point = exponent as usize + 1;
    match digits.split_at_checked(point) {
        Some((whole, "")) => format!("{sign}{whole}"),
        Some((whole, rest)) => format!("{sign}{whole}.{rest}"),
        None => {
            let padding = point - digits.len();
            format!("{sign}{digits}{}", "0".repeat(padding))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_spell_like_the_recorded_hashes() {
        let cases = [
            ("0", "0"),
            ("-0.0", "0"),
            ("1.0", "1"),
            ("0.1", "0.1"),
            ("-1.5", "-1.5"),
            ("1e21", "1e+21"),
            ("1e-7", "1e-7"),
            ("1.5e-7", "1.5e-7"),
            ("1e20", "100000000000000000000"),
            ("1e-6", "0.000001"),
            ("12345678901234567890", "12345678901234567000"),
        ];
        for (input, expected) in cases {
            let number: f64 = input.parse().unwrap();
            assert_eq!(spell_number(number), expected, "of {input}");
        }
    }

    #[test]
    fn members_keep_document_order_and_integer_keys_sort_first() {
        let cases = [
            (r#"{"b":1,"2":2,"a":3}"#, r#"{"2":2,"b":1,"a":3}"#),
            (
                r#"{"9":null,"10":true,"z":1}"#,
                r#"{"9":null,"10":true,"z":1}"#,
            ),
            (
                r#"{"a":[1,{"b":null},true]}"#,
                r#"{"a":[1,{"b":null},true]}"#,
            ),
            (
                r#"{"s":"quote\"back\\slash"}"#,
                r#"{"s":"quote\"back\\slash"}"#,
            ),
            (r#"{"a":1,"a":2}"#, r#"{"a":2}"#),
        ];
        for (input, expected) in cases {
            assert_eq!(spelling(&parse(input).unwrap()), expected, "of {input}");
        }
    }

    #[test]
    fn parse_reports_decode_failures() {
        let error = parse("{").expect_err("truncated document is rejected");
        assert!(error.to_string().contains("decode failed"));
        let error = parse("{} {}").expect_err("trailing characters are rejected");
        assert!(error.to_string().contains("decode failed"));
    }

    #[test]
    fn conversions_are_typed_and_report_failures() {
        let object: JsonObject = serde_json::from_str(r#"{"a":1}"#).unwrap();
        let value = parse(r#"{"a":1}"#).unwrap();
        assert_eq!(spelling(&value), r#"{"a":1}"#);
        let back = to_object(&value).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), r#"{"a":1}"#);
        assert_eq!(object_bytes(&object).unwrap(), 7);
        assert_eq!(object_chars(&object).unwrap(), 7);
        assert!(decode::<Vec<String>>(&object).is_err());
        let JsJson::Object(members) = value else {
            panic!("parsed object members");
        };
        assert_eq!(members_of(&object).unwrap(), members);
    }

    #[test]
    fn members_are_edited_in_place() {
        let mut members = vec![
            ("keep".to_owned(), JsJson::String("yes".to_owned())),
            ("drop".to_owned(), JsJson::Bool(true)),
        ];
        members.retain(|(key, _)| key == "keep");
        set_member(&mut members, "added", number(11));
        set_member(&mut members, "keep", JsJson::Bool(false));
        assert_eq!(
            spelling(&JsJson::Object(members)),
            r#"{"keep":false,"added":11}"#
        );
        let empty: JsonObject = serde_json::from_str("{}").unwrap();
        assert_eq!(members_of(&empty).unwrap(), [] as [(String, JsJson); 0]);
    }
}
