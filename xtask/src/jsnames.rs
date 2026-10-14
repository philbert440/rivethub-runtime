use std::path::Path;

use crate::baseline::Baseline;
use crate::error::Result;

pub fn check(root: &Path, baseline: &Baseline, findings: &mut Vec<String>) -> Result<()> {
    crate::rustsrc::check(root, baseline, "JSMODEL", findings)
}

pub fn banned_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("js_") {
        return true;
    }
    let mut chars = name.chars();
    if !matches!((chars.next(), chars.next()), (Some('J'), Some('s'))) {
        return false;
    }
    chars.next().is_some_and(|third| third.is_ascii_uppercase())
}
