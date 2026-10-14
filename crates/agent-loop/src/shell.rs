use std::collections::BTreeSet;

use regex::RegexSet;
use serde::Deserialize;
use wire::JsonObject;

use crate::hooks::HookError;

#[derive(Debug)]
pub struct ShellDanger {
    tools: BTreeSet<String>,
    patterns: RegexSet,
}

#[derive(Deserialize)]
struct ShellArguments {
    command: Option<String>,
}

impl ShellDanger {
    pub fn new(tools: BTreeSet<String>) -> Result<Self, HookError> {
        Self::with_patterns(
            tools,
            [
                r"(?i)(^|[\s;/|&(`])rm\s+[^\n]*(--recursive|-[a-z]*r)",
                r"(?i)(^|[\s;/|&(`])(dd|mkfs(?:\.[a-z0-9]+)?|shred|wipefs|fdisk|parted)(\s|$)",
                r":\s*\(\s*\)\s*\{",
                r"(?i)\|\s*(?:/\S*/)?(?:ba|da|z|k)?sh(\s|$)",
                r"(?i)(^|[\s;|&(`])(?:sudo|su|eval|exec)(\s|$)",
                r"(?i)>\s*/dev/(?:sd|hd|vd|xvd|nvme|mmcblk|disk|mapper/)",
                r"(?i)(^|[\s;/|&(`])(?:ba|da|z|k)?sh\s+[^\n]*-c",
            ],
        )
    }

    pub fn with_patterns<I, S>(tools: BTreeSet<String>, patterns: I) -> Result<Self, HookError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let patterns = RegexSet::new(patterns)?;
        if patterns.is_empty() {
            return Err(HookError::EmptyDangerPolicy);
        }
        Ok(Self { tools, patterns })
    }

    pub fn check(&self, tool: &str, args: &JsonObject) -> Result<(), HookError> {
        if !self.tools.contains(tool) {
            return Ok(());
        }
        let args: ShellArguments = serde_json::from_value(serde_json::to_value(args)?)?;
        if let Some(command) = args.command
            && self.patterns.is_match(&command)
        {
            return Err(HookError::DangerousShell);
        }
        Ok(())
    }
}
