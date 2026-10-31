use std::future::Future;

use uuid::Uuid;

tokio::task_local! {
    static ORIGIN: Uuid;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteOrigin;

impl NoteOrigin {
    pub async fn run<F: Future>(summary: Uuid, work: F) -> F::Output {
        ORIGIN.scope(summary, work).await
    }

    #[must_use]
    pub fn current() -> Option<Uuid> {
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                fallback,
                reason = "PATTERNS FALLBACK: a real absence, as page syncs and edits write outside any summary's notes"
            )
        )]
        let current = ORIGIN.try_with(|summary| *summary).ok();
        current
    }
}
