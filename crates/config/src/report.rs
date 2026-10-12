use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub path: String,
    pub message: String,
}

impl Finding {
    pub fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

impl ValidationResult {
    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn push(&mut self, finding: Finding) {
        match finding.severity {
            Severity::Error => self.errors.push(finding),
            Severity::Warning => self.warnings.push(finding),
        }
    }

    pub fn error(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.push(Finding::error(path, message));
    }

    pub fn warning(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.push(Finding::warning(path, message));
    }
}

impl std::fmt::Display for ValidationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut blocks: Vec<String> = Vec::new();
        if !self.errors.is_empty() {
            let mut block = String::from("Errors:");
            for finding in &self.errors {
                block.push_str("\n  \u{274c} ");
                push_finding(&mut block, finding);
            }
            blocks.push(block);
        }
        if !self.warnings.is_empty() {
            let mut block = String::from("Warnings:");
            for finding in &self.warnings {
                block.push_str("\n  \u{26a0}\u{fe0f}  ");
                push_finding(&mut block, finding);
            }
            blocks.push(block);
        }
        let mut text = blocks.join("\n\n");
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&footer(self));
        f.write_str(&text)
    }
}

fn push_finding(text: &mut String, finding: &Finding) {
    if !finding.path.is_empty() {
        text.push('[');
        text.push_str(&finding.path);
        text.push_str("] ");
    }
    text.push_str(&finding.message);
}

fn footer(result: &ValidationResult) -> String {
    let errors = result.errors.len();
    let warnings = result.warnings.len();
    if errors == 0 && warnings == 0 {
        return "\u{2705} Config is valid.".to_owned();
    }
    if errors == 0 {
        return format!(
            "\u{2705} Config is valid ({} {}).",
            warnings,
            plural("warning", warnings)
        );
    }
    if warnings == 0 {
        return format!(
            "\u{274c} Config has {} {}.",
            errors,
            plural("error", errors)
        );
    }
    format!(
        "\u{274c} Config has {} {} and {} {}.",
        errors,
        plural("error", errors),
        warnings,
        plural("warning", warnings)
    )
}

fn plural(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}
