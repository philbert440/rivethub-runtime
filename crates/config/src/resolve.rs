use crate::environment::Environment;

pub fn resolve_env_vars_with(env: &Environment, value: &mut serde_yaml_ng::Value) {
    match value {
        serde_yaml_ng::Value::String(text) => *text = substitute(env, text),
        serde_yaml_ng::Value::Sequence(items) => {
            for item in items {
                resolve_env_vars_with(env, item);
            }
        }
        serde_yaml_ng::Value::Mapping(entries) => {
            for (_key, item) in entries.iter_mut() {
                resolve_env_vars_with(env, item);
            }
        }
        serde_yaml_ng::Value::Tagged(tagged) => resolve_env_vars_with(env, &mut tagged.value),
        serde_yaml_ng::Value::Null
        | serde_yaml_ng::Value::Bool(_)
        | serde_yaml_ng::Value::Number(_) => {}
    }
}

fn unset_is_empty<'a>(env: &'a Environment, name: &str) -> &'a str {
    env.text(name).unwrap_or("")
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
            out.push_str(unset_is_empty(env, &name));
        } else {
            out.push_str("${");
            out.push_str(&name);
        }
    }
    out
}
