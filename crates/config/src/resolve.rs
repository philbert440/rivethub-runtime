use crate::environment::Environment;

pub fn resolve_env_vars_with(env: &Environment, value: &serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::String(text) => serde_yaml::Value::String(substitute(env, text)),
        serde_yaml::Value::Sequence(items) => serde_yaml::Value::Sequence(
            items
                .iter()
                .map(|item| resolve_env_vars_with(env, item))
                .collect(),
        ),
        serde_yaml::Value::Mapping(entries) => serde_yaml::Value::Mapping(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), resolve_env_vars_with(env, item)))
                .collect(),
        ),
        serde_yaml::Value::Tagged(tagged) => {
            serde_yaml::Value::Tagged(Box::new(serde_yaml::value::TaggedValue {
                tag: tagged.tag.clone(),
                value: resolve_env_vars_with(env, &tagged.value),
            }))
        }
        serde_yaml::Value::Null | serde_yaml::Value::Bool(_) | serde_yaml::Value::Number(_) => {
            value.clone()
        }
    }
}

fn substitute(env: &Environment, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '$' || chars.peek().map(|(_, c)| *c) != Some('{') {
            out.push(ch);
            continue;
        }
        chars.next();
        let mut name = String::new();
        loop {
            match chars.peek().copied() {
                Some((_, c)) if c.is_ascii_alphanumeric() || c == '_' => {
                    name.push(c);
                    chars.next();
                }
                _ => break,
            }
        }
        if !name.is_empty() && matches!(chars.peek().copied(), Some((_, '}'))) {
            chars.next();
            out.push_str(env.get(&name).unwrap_or(""));
        } else {
            out.push_str("${");
            out.push_str(&name);
        }
    }
    out
}
