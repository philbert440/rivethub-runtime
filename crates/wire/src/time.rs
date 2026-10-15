use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde_with::DeserializeAs;
use serde_with::SerializeAs;

#[derive(Debug)]
pub struct Millis;

impl SerializeAs<DateTime<Utc>> for Millis {
    fn serialize_as<S>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

impl<'de> DeserializeAs<'de, DateTime<Utc>> for Millis {
    fn deserialize_as<D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&raw)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}
