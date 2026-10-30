use std::collections::BTreeMap;

use wire::plugin::host::PluginSettings;
use wire::plugin::view::{ViewField, ViewInput, ViewValue};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{key}: {message}")]
pub(super) struct SettingProblem {
    pub(super) key: String,
    pub(super) message: String,
}

fn refused(key: &str, message: impl Into<String>) -> SettingProblem {
    SettingProblem {
        key: key.to_owned(),
        message: message.into(),
    }
}

pub(super) fn effective(
    fields: &[ViewField],
    kept: &BTreeMap<String, ViewValue>,
) -> BTreeMap<String, ViewValue> {
    fields
        .iter()
        .filter_map(|field| {
            kept.get(&field.key)
                .or(field.value.as_ref())
                .map(|value| (field.key.clone(), value.clone()))
        })
        .collect()
}

pub(super) fn shown(fields: &[ViewField], kept: &BTreeMap<String, ViewValue>) -> PluginSettings {
    let mut values = effective(fields, kept);
    let secrets_set = fields
        .iter()
        .filter(|field| matches!(field.input, ViewInput::Secret))
        .filter_map(|field| values.remove(&field.key).map(|_| field.key.clone()))
        .collect();
    PluginSettings {
        values,
        secrets_set,
    }
}

pub(super) fn apply(
    fields: &[ViewField],
    kept: &BTreeMap<String, ViewValue>,
    update: &BTreeMap<String, Option<ViewValue>>,
) -> Result<BTreeMap<String, ViewValue>, SettingProblem> {
    let mut next: BTreeMap<String, ViewValue> = kept
        .iter()
        .filter(|(key, _)| fields.iter().any(|field| &field.key == *key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, value) in update {
        let field = fields
            .iter()
            .find(|field| &field.key == key)
            .ok_or_else(|| refused(key, "is not one of the plug-in's settings"))?;
        match value {
            Some(value) => {
                fits(&field.input, value).map_err(|message| refused(key, message))?;
                next.insert(key.clone(), value.clone());
            }
            None => {
                next.remove(key);
            }
        }
    }
    let values = effective(fields, &next);
    if let Some(field) = fields
        .iter()
        .find(|field| field.required && !values.contains_key(&field.key))
    {
        return Err(refused(&field.key, "is required"));
    }
    Ok(next)
}

fn fits(input: &ViewInput, value: &ViewValue) -> Result<(), String> {
    match (input, value) {
        (ViewInput::Text { .. } | ViewInput::Secret, ViewValue::Text(_))
        | (ViewInput::Toggle, ViewValue::Bool(_)) => Ok(()),
        (ViewInput::Number { min, max, .. }, ViewValue::Number(number)) => {
            if !number.is_finite() {
                return Err("must be a number".to_owned());
            }
            if let Some(min) = min
                && number < min
            {
                return Err(format!("must be at least {min}"));
            }
            if let Some(max) = max
                && number > max
            {
                return Err(format!("must be at most {max}"));
            }
            Ok(())
        }
        (
            ViewInput::Select {
                options,
                multiple: false,
            },
            ViewValue::Text(chosen),
        ) => offered(options, chosen),
        (
            ViewInput::Select {
                options,
                multiple: true,
            },
            ViewValue::List(chosen),
        ) => chosen
            .iter()
            .try_for_each(|chosen| offered(options, chosen)),
        (ViewInput::Text { .. } | ViewInput::Secret, _) => Err("must be text".to_owned()),
        (ViewInput::Number { .. }, _) => Err("must be a number".to_owned()),
        (ViewInput::Toggle, _) => Err("must be true or false".to_owned()),
        (
            ViewInput::Select {
                multiple: false, ..
            },
            _,
        ) => Err("must be one option".to_owned()),
        (ViewInput::Select { multiple: true, .. }, _) => {
            Err("must be a list of options".to_owned())
        }
        (ViewInput::File { .. }, _) => Err("a setting cannot be a file".to_owned()),
    }
}

fn offered(options: &[wire::plugin::view::ViewOption], chosen: &str) -> Result<(), String> {
    if options.iter().any(|option| option.value == chosen) {
        Ok(())
    } else {
        Err(format!("{chosen} is not one of its options"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wire::plugin::view::{ViewField, ViewValue};

    use super::{apply, effective, shown};

    #[test]
    fn settings_are_checked_against_their_fields() -> Result<(), serde_json::Error> {
        let fields: Vec<ViewField> = serde_json::from_str(
            r#"[{"key": "runs", "label": "Runs", "input": {"type": "number", "min": 1, "max": 8}, "value": 2, "required": true},
                {"key": "mode", "label": "Mode", "input": {"type": "select", "options": [{"value": "fast", "label": "Fast"}]}},
                {"key": "token", "label": "Token", "input": {"type": "secret"}}]"#,
        )?;
        let none = BTreeMap::new();
        let update = |pairs: &[(&str, Option<ViewValue>)]| -> BTreeMap<String, Option<ViewValue>> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect()
        };
        let refused = |pairs: &[(&str, Option<ViewValue>)]| {
            apply(&fields, &none, &update(pairs)).map_err(|problem| problem.key)
        };
        assert_eq!(
            refused(&[("runs", Some(ViewValue::Number(9.0)))]),
            Err("runs".to_owned())
        );
        assert_eq!(
            refused(&[("runs", Some(ViewValue::Bool(true)))]),
            Err("runs".to_owned())
        );
        assert_eq!(
            refused(&[("mode", Some(ViewValue::Text("slow".into())))]),
            Err("mode".to_owned())
        );
        assert_eq!(
            refused(&[("colour", Some(ViewValue::Text("red".into())))]),
            Err("colour".to_owned())
        );
        let kept = apply(
            &fields,
            &none,
            &update(&[
                ("runs", None),
                ("token", Some(ViewValue::Text("s3cret".into()))),
            ]),
        )
        .map_err(|problem| serde::de::Error::custom(problem.to_string()))?;
        assert_eq!(
            effective(&fields, &kept).get("runs"),
            Some(&ViewValue::Number(2.0))
        );
        let visible = shown(&fields, &kept);
        assert_eq!(visible.secrets_set, ["token"]);
        assert!(!visible.values.contains_key("token"));
        Ok(())
    }
}
