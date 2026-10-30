use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

use crate::filing::Refusal;
use crate::topics::TopicName;
use crate::topics::fold;

const MIN_STEM_CHARS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum JunkReason {
    Generic,
    Rejected,
    Title,
    Task,
    Activity,
    Vague,
    Refused { refusal: Refusal },
    Duplicate { of: String },
    TooDeep,
    TooFew { sessions: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum JunkStage {
    Tree,
    Filing,
    Fold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Junk {
    pub name: String,
    pub stage: JunkStage,
    #[serde(flatten)]
    pub reason: JunkReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum GenericName {
    General,
    Misc,
    Miscellaneous,
    Other,
    Others,
    Uncategorized,
    Various,
    Notes,
    Chat,
    Chores,
    Questions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum TaskVerb {
    Add,
    Check,
    Debug,
    Deploy,
    Fix,
    Implement,
    Install,
    Investigate,
    Migrate,
    Remove,
    Rename,
    Review,
    Run,
    Test,
    Update,
    Upgrade,
    Verify,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum ActivityWord {
    Review,
    Reviews,
    Fix,
    Fixes,
    Fixing,
    Task,
    Tasks,
    Punchlist,
    Punchlists,
    Punch,
    Ops,
    Operations,
    Issue,
    Issues,
    Resolution,
    Triage,
    Audit,
    Audits,
    Bugfix,
    Bugfixes,
    Smoke,
    Chores,
    Cleanup,
    Maintenance,
    Job,
    Jobs,
    Slice,
    Slices,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum VagueWord {
    Den,
    Program,
    Programme,
    Stuff,
    Things,
    Workstream,
    Initiative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum FillerWord {
    A,
    An,
    And,
    For,
    In,
    Of,
    On,
    The,
    To,
    With,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameGuard {
    titles: BTreeSet<String>,
    rejected: BTreeSet<String>,
    context: BTreeSet<String>,
}

impl NameGuard {
    pub fn new<'a>(
        titles: impl IntoIterator<Item = &'a str>,
        rejected: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Self {
            titles: titles.into_iter().map(fold).collect(),
            rejected: rejected.into_iter().map(fold).collect(),
            context: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn with_context<'a>(mut self, names: impl IntoIterator<Item = &'a str>) -> Self {
        self.context = names
            .into_iter()
            .map(fold)
            .filter(|name| !name.is_empty() && !name.contains('-'))
            .map(|name| stem(&name))
            .collect();
        self
    }

    #[must_use]
    pub fn head(&self, name: &str) -> Option<String> {
        fold(name)
            .split('-')
            .filter(|word| !word.is_empty() && word.parse::<FillerWord>().is_err())
            .map(stem)
            .find(|word| !self.context.contains(word))
    }

    #[must_use]
    pub fn twin(&self, left: &str, right: &str) -> bool {
        let distinct = |name: &str| -> BTreeSet<String> {
            stems(name)
                .into_iter()
                .filter(|word| !self.context.contains(word))
                .collect()
        };
        let (left, right) = (distinct(left), distinct(right));
        let (small, large) = if left.len() <= right.len() {
            (&left, &right)
        } else {
            (&right, &left)
        };
        !small.is_empty() && small.is_subset(large)
    }

    #[must_use]
    pub fn junk(&self, name: &TopicName) -> Option<JunkReason> {
        let folded = fold(name.as_str());
        if self.rejected.contains(&folded) {
            return Some(JunkReason::Rejected);
        }
        if folded.parse::<GenericName>().is_ok() {
            return Some(JunkReason::Generic);
        }
        if self.titles.contains(&folded) {
            return Some(JunkReason::Title);
        }
        if let Some(refusal) = Refusal::of(name, None) {
            return Some(JunkReason::Refused { refusal });
        }
        let task = name
            .as_str()
            .split_whitespace()
            .next()
            .is_some_and(|first| first.parse::<TaskVerb>().is_ok());
        if task {
            return Some(JunkReason::Task);
        }
        let words: Vec<&str> = folded.split('-').collect();
        if words
            .iter()
            .any(|word| word.parse::<ActivityWord>().is_ok())
        {
            return Some(JunkReason::Activity);
        }
        words
            .iter()
            .any(|word| word.parse::<VagueWord>().is_ok())
            .then_some(JunkReason::Vague)
    }
}

fn stems(name: &str) -> BTreeSet<String> {
    fold(name)
        .split('-')
        .filter(|word| !word.is_empty() && word.parse::<FillerWord>().is_err())
        .map(stem)
        .collect()
}

fn stem(word: &str) -> String {
    let long = |rest: &&str| rest.chars().count() >= MIN_STEM_CHARS;
    if let Some(rest) = word.strip_suffix("ies").filter(long) {
        return format!("{rest}y");
    }
    ["ing", "s", "ed"]
        .into_iter()
        .find_map(|suffix| word.strip_suffix(suffix).filter(long))
        .unwrap_or(word)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> NameGuard {
        NameGuard::new(["Fix the AcmeKit login page"], ["scratchpad"])
    }

    fn junk(name: &str) -> Option<JunkReason> {
        guard().junk(&TopicName::new(name).unwrap())
    }

    #[test]
    fn refuses_names_that_are_not_lasting_areas() {
        assert_eq!(junk("General"), Some(JunkReason::Generic));
        assert_eq!(junk("scratchpad"), Some(JunkReason::Rejected));
        assert_eq!(junk("Fix the AcmeKit login page"), Some(JunkReason::Title));
        assert_eq!(junk("Fix login flow"), Some(JunkReason::Task));
        assert_eq!(
            junk("acmekit v2"),
            Some(JunkReason::Refused {
                refusal: Refusal::Version
            })
        );
        assert_eq!(
            junk("feat-login"),
            Some(JunkReason::Refused {
                refusal: Refusal::Branch
            })
        );
        assert_eq!(junk("AcmeKit Releases"), None);
        assert_eq!(junk("Code Review Program"), Some(JunkReason::Activity));
        assert_eq!(junk("Den Server API"), Some(JunkReason::Vague));
        assert_eq!(junk("families.app"), None);
    }

    #[test]
    fn twins_share_their_distinctive_word_stems() {
        let guard = guard().with_context(["AcmeKit", "Desktop App", "app"]);
        assert!(guard.twin("Model Benchmarks", "model benchmarking"));
        assert!(guard.twin("Library Upgrades", "libraries upgrade"));
        assert!(guard.twin("Memory System", "Memory Wiki System"));
        assert!(guard.twin("AcmeKit Android App", "Android Client"));
        assert!(!guard.twin("Release Pipeline", "Release Notes"));
        assert!(!guard.twin("AcmeKit Desktop", "AcmeKit"));
        assert!(!guard.twin("Local Mode", "Session Modes"));
    }
}
