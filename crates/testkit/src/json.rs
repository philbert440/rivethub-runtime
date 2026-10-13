use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::Mutex;
use regex::Regex;

#[rustfmt::skip]
pub type Json = serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Masks {
    pub port: String,
    pub hostname: Option<String>,
    pub tmp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenClass {
    Date,
    Timestamp,
    Port,
    Tmp,
    Hostname,
    EpochMs,
    Repo,
    Node,
    NodeVersion,
    Key,
    GenId,
    Pty { id: u32 },
    Pid { id: u32 },
}

impl TokenClass {
    const DATE_PATTERN: &'static str =
        "[A-Za-z]{3}, [0-9]{2} [A-Za-z]{3} [0-9]{4} [0-9]{2}:[0-9]{2}:[0-9]{2} GMT";
    const TIMESTAMP_PATTERN: &'static str = "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:[.][0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})";
    const DIGITS_PATTERN: &'static str = "[0-9]+";
    const TMP_PATTERN: &'static str = r#"[^ "\\)\]]{1,1024}"#;
    const HOSTNAME_PATTERN: &'static str = r#"[^"\\]{1,256}"#;
    const EPOCH_MS_PATTERN: &'static str = "[0-9]{13}";
    const PATHISH_PATTERN: &'static str = r#"[^"\\]{1,1024}"#;
    const NODE_VERSION_PATTERN: &'static str = "v?[0-9]+[.][0-9]+[.][0-9]+";
    const KEY_PATTERN: &'static str = "[0-9]{13}-[0-9a-z]+";
    const GENID_PATTERN: &'static str = "(?:aitxt-[0-9A-Za-z]{24,}|[0-9A-Za-z]{16,})";
    const PTY_PATTERN: &'static str = "pty-[0-9a-f]{8}";

    fn named(name: &str) -> Option<Self> {
        match name {
            "DATE" => Some(Self::Date),
            "TS" | "GENTS" => Some(Self::Timestamp),
            "PORT" => Some(Self::Port),
            "TMP" => Some(Self::Tmp),
            "HOSTNAME" => Some(Self::Hostname),
            "EPOCH_MS" => Some(Self::EpochMs),
            "REPO" => Some(Self::Repo),
            "NODE" => Some(Self::Node),
            "NODE_VERSION" => Some(Self::NodeVersion),
            "KEY" => Some(Self::Key),
            "GENID" => Some(Self::GenId),
            _ => None,
        }
    }

    fn numbered(name: &str) -> Option<Self> {
        let (prefix, number) = name.split_once('_')?;
        let id = number.parse::<u32>().ok()?;
        match prefix {
            "PTY" => Some(Self::Pty { id }),
            "PID" => Some(Self::Pid { id }),
            _ => None,
        }
    }

    fn pattern(self) -> &'static str {
        match self {
            Self::Date => Self::DATE_PATTERN,
            Self::Timestamp => Self::TIMESTAMP_PATTERN,
            Self::Port | Self::Pid { .. } => Self::DIGITS_PATTERN,
            Self::Tmp => Self::TMP_PATTERN,
            Self::Hostname => Self::HOSTNAME_PATTERN,
            Self::EpochMs => Self::EPOCH_MS_PATTERN,
            Self::Repo | Self::Node => Self::PATHISH_PATTERN,
            Self::NodeVersion => Self::NODE_VERSION_PATTERN,
            Self::Key => Self::KEY_PATTERN,
            Self::GenId => Self::GENID_PATTERN,
            Self::Pty { .. } => Self::PTY_PATTERN,
        }
    }

    fn concrete(self, masks: &Masks) -> Option<String> {
        match self {
            Self::Port => Some(masks.port.clone()),
            Self::Tmp => masks.tmp.clone(),
            Self::Hostname => masks.hostname.clone(),
            Self::Date
            | Self::Timestamp
            | Self::EpochMs
            | Self::Repo
            | Self::Node
            | Self::NodeVersion
            | Self::Key
            | Self::GenId
            | Self::Pty { .. }
            | Self::Pid { .. } => None,
        }
    }
}

#[expect(
    clippy::expect_used,
    reason = "the token pattern is a constant; the mask unit tests match it"
)]
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{[^{}]*\}\}|<ts>").expect("the token pattern is a constant"));

static PATTERN_CACHE: LazyLock<Mutex<HashMap<String, Regex>>> = LazyLock::new(Mutex::default);

fn token_regex() -> &'static Regex {
    &TOKEN
}

#[expect(
    clippy::expect_used,
    reason = "the composed source is built from constant class patterns plus regex::escape'd literals; the mask unit tests compile every class"
)]
fn compiled(source: &str) -> Regex {
    let mut cache = PATTERN_CACHE.lock();
    if let Some(built) = cache.get(source) {
        return built.clone();
    }
    let built = Regex::new(source).expect("composed mask patterns are built from constants");
    cache.insert(source.to_string(), built.clone());
    built
}

struct Masked {
    source: String,
    numbered: Vec<(TokenClass, String)>,
}

fn masked_pattern(rec: &str, masks: &Masks) -> Option<Masked> {
    let finder = token_regex();
    let mut source = String::with_capacity(rec.len() + 32);
    source.push_str(r"\A");
    let mut numbered = Vec::new();
    let mut cursor = 0;
    for found in finder.find_iter(rec) {
        source.push_str(&regex::escape(rec.get(cursor..found.start())?));
        cursor = found.end();
        let token = found.as_str();
        let named = token
            .strip_prefix("{{")
            .and_then(|rest| rest.strip_suffix("}}"))
            .map(str::trim);
        let class = named.map_or_else(
            || Some(TokenClass::Timestamp),
            |name| TokenClass::named(name).or_else(|| TokenClass::numbered(name)),
        );
        let Some(class) = class else {
            source.push_str(&regex::escape(token));
            continue;
        };
        if let Some(text) = class.concrete(masks) {
            source.push_str(&regex::escape(&text));
            continue;
        }
        match class {
            TokenClass::Pty { .. } | TokenClass::Pid { .. } => {
                let group = format!("t{}", numbered.len() + 1);
                source.push_str("(?P<");
                source.push_str(&group);
                source.push('>');
                source.push_str(class.pattern());
                source.push(')');
                numbered.push((class, group));
            }
            TokenClass::Date
            | TokenClass::Timestamp
            | TokenClass::Port
            | TokenClass::Tmp
            | TokenClass::Hostname
            | TokenClass::EpochMs
            | TokenClass::Repo
            | TokenClass::Node
            | TokenClass::NodeVersion
            | TokenClass::Key
            | TokenClass::GenId => source.push_str(class.pattern()),
        }
    }
    source.push_str(&regex::escape(rec.get(cursor..)?));
    source.push_str(r"\z");
    Some(Masked { source, numbered })
}

fn numbered_consistent(captures: &regex::Captures<'_>, numbered: &[(TokenClass, String)]) -> bool {
    let mut bound: Vec<(TokenClass, &str)> = Vec::new();
    for (class, group) in numbered {
        let Some(text) = captures.name(group) else {
            return false;
        };
        let text = text.as_str();
        if let Some((_, seen)) = bound.iter().find(|(bound_class, _)| bound_class == class) {
            if *seen != text {
                return false;
            }
        } else {
            if bound.iter().any(|(_, seen)| *seen == text) {
                return false;
            }
            bound.push((*class, text));
        }
    }
    true
}

#[must_use]
pub fn masked_eq(rec: &str, act: &str, masks: &Masks) -> bool {
    if !has_tokens(rec) {
        return rec == act;
    }
    let Some(masked) = masked_pattern(rec, masks) else {
        return false;
    };
    let regex = compiled(&masked.source);
    let Some(captures) = regex.captures(act) else {
        return false;
    };
    numbered_consistent(&captures, &masked.numbered)
}

#[must_use]
pub fn has_tokens(rec: &str) -> bool {
    rec.contains("{{") || rec.contains("<ts>")
}

fn num_eq(a: &Json, b: &Json) -> bool {
    if a == b {
        return true;
    }
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn value_eq(rec: &Json, act: &Json, strings: &dyn Fn(&str, &str) -> bool) -> bool {
    match (rec, act) {
        (Json::Object(r), Json::Object(a)) => {
            r.iter().filter(|(_, v)| !v.is_null()).all(|(k, rv)| {
                a.get(k)
                    .is_some_and(|av| !av.is_null() && value_eq(rv, av, strings))
            }) && a.iter().filter(|(_, v)| !v.is_null()).all(|(k, av)| {
                r.get(k)
                    .is_some_and(|rv| !rv.is_null() && value_eq(rv, av, strings))
            })
        }
        (Json::Array(r), Json::Array(a)) => {
            r.len() == a.len() && r.iter().zip(a).all(|(x, y)| value_eq(x, y, strings))
        }
        (Json::String(r), Json::String(a)) => strings(r, a),
        (Json::Number(_), Json::Number(_)) => num_eq(rec, act),
        (Json::Bool(r), Json::Bool(a)) => r == a,
        (Json::Null, Json::Null) => true,
        _ => false,
    }
}

#[must_use]
pub fn json_eq(a: &Json, b: &Json) -> bool {
    value_eq(a, b, &|x, y| x == y)
}

#[must_use]
pub fn json_masked_eq(rec: &Json, act: &Json, masks: &Masks) -> bool {
    value_eq(rec, act, &|r, a| masked_eq(r, a, masks))
}

#[must_use]
pub fn substitute(text: &str, masks: &Masks) -> String {
    let mut out = text.replace("{{PORT}}", &masks.port);
    if let Some(tmp) = &masks.tmp {
        out = out.replace("{{TMP}}", tmp);
    }
    if let Some(host) = &masks.hostname {
        out = out.replace("{{HOSTNAME}}", host);
    }
    out
}
