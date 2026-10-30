use std::collections::BTreeMap;

use wire::plugin::host::PluginSettings;
use wire::plugin::view::{ViewField, ViewInput, ViewValue};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{key}: {message}")]
pub struct SettingProblem {
    pub key: String,
    pub message: String,
}

fn refused(key: &str, message: impl Into<String>) -> SettingProblem {
    SettingProblem {
        key: key.to_owned(),
        message: message.into(),
    }
}

#[must_use]
pub fn effective(
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

#[must_use]
pub fn shown(fields: &[ViewField], kept: &BTreeMap<String, ViewValue>) -> PluginSettings {
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

pub fn apply(
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
