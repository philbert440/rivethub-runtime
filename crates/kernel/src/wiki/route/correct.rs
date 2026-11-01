use super::TopicTarget;
use super::WikiRouter;
use crate::wiki::builder::WikiSummary;
use crate::wiki::merge::Correction;
use crate::wiki::page::WikiPatch;
use crate::wiki::page::bound::blocks;
use crate::wiki::page::lead::rewords;
use crate::wiki::page::lead::split_lead;

impl WikiRouter<'_> {
    pub(super) async fn corrected(
        &self,
        topic: &TopicTarget,
        mut patch: WikiPatch,
        summary: &WikiSummary,
        also: &[WikiSummary],
    ) -> WikiPatch {
        let (Some(merger), Some(current), true) = (
            self.merger,
            topic.current.as_ref(),
            patch.replaces_description,
        ) else {
            return patch;
        };
        let lead = match &topic.section {
            Some(_) => current
                .section
                .as_deref()
                .and_then(|section| blocks(section).into_iter().next())
                .unwrap_or_default(),
            None => current.summary.clone(),
        };
        let old = split_lead(&lead).lead;
        let new = split_lead(&patch.summary);
        if old.is_empty() || !rewords(&old, &new.lead) {
            return patch;
        }
        let correction = Correction {
            description: old,
            summaries: std::iter::once(summary)
                .chain(also)
                .map(|source| source.content.clone())
                .collect(),
        };
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                fallback,
                reason = "PATTERNS FALLBACK: the rule of contract note 119: a failed correction keeps the description the extraction wrote"
            )
        )]
        let Ok(Some(description)) = merger.correct(&correction).await else {
            return patch;
        };
        patch.summary = match new.currently {
            Some(currently) => format!("{} {currently}", description.trim()),
            None => description.trim().to_owned(),
        };
        patch
    }
}
