use regex::Captures;

use crate::privacy::{allowed_home_users, res};

pub const HOME_PATH: &str = "home-path";
pub const LAN_HOST: &str = "lan-host";
pub const IP: &str = "ip";

const HOME_MASK: &str = "[masked home-path]";
const HOST_MASK: &str = "[masked host]";
const IP_MASK: &str = "[masked ip]";

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Masked {
    pub file: &'static str,
    pub rule: &'static str,
    pub matched: String,
}

pub fn mask_document(file: &'static str, text: &str) -> (String, Vec<Masked>) {
    let mut masked = String::with_capacity(text.len());
    let mut hits = Vec::new();
    for line in text.split_inclusive('\n') {
        let (masked_line, line_hits) = mask_line(line);
        masked.push_str(&masked_line);
        hits.extend(line_hits.into_iter().map(|(rule, matched)| Masked {
            file,
            rule,
            matched,
        }));
    }
    (masked, hits)
}

fn mask_line(line: &str) -> (String, Vec<(&'static str, String)>) {
    let mut hits = Vec::new();
    let rules = &res();
    let mut out = replace_matched(
        line,
        &rules.home,
        HOME_MASK,
        HOME_PATH,
        &mut hits,
        home_is_foreign,
    );
    out = replace_matched(
        &out,
        &rules.lan_host,
        HOST_MASK,
        LAN_HOST,
        &mut hits,
        |_| true,
    );
    out = replace_matched(&out, &rules.ipv6_internal, IP_MASK, IP, &mut hits, |_| true);
    out = replace_matched(&out, &rules.ipv4_full, IP_MASK, IP, &mut hits, |_| true);
    (out, hits)
}

fn home_is_foreign(captures: &Captures<'_>) -> bool {
    captures
        .get(1)
        .is_none_or(|user| !allowed_home_users().contains(&user.as_str()))
}

fn replace_matched(
    line: &str,
    pattern: &regex::Regex,
    mask: &'static str,
    rule: &'static str,
    hits: &mut Vec<(&'static str, String)>,
    matches_rule: fn(&Captures<'_>) -> bool,
) -> String {
    pattern
        .replace_all(line, |captures: &Captures<'_>| {
            if !matches_rule(captures) {
                return whole(captures);
            }
            hits.push((rule, whole(captures)));
            mask.to_owned()
        })
        .into_owned()
}

fn whole(captures: &Captures<'_>) -> String {
    captures
        .get(0)
        .map_or(String::new(), |found| found.as_str().to_owned())
}
