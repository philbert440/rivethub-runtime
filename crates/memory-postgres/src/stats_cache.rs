use std::collections::BTreeMap;
use std::time::Duration;
use std::time::Instant;

use kernel::memory::MemoryStatsResponse;
use kernel::tenancy::Scope;
use parking_lot::Mutex;

const FRESH_FOR: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub(crate) struct StatsCache {
    answers: Mutex<BTreeMap<String, (Instant, MemoryStatsResponse)>>,
}

fn key() -> String {
    match Scope::current() {
        Some(Scope::Viewer(viewer)) => {
            let spaces: Vec<&str> = viewer
                .spaces()
                .iter()
                .map(kernel::tenancy::SpaceId::as_str)
                .collect();
            format!(
                "viewer:{}:{}",
                viewer.principal().as_str(),
                spaces.join(",")
            )
        }
        Some(Scope::Maintenance) => "maintenance".to_owned(),
        Some(Scope::Closed) | None => "closed".to_owned(),
    }
}

impl StatsCache {
    pub(crate) fn fresh(&self) -> Option<MemoryStatsResponse> {
        let key = key();
        let answers = self.answers.lock();
        answers
            .get(&key)
            .filter(|(at, _)| at.elapsed() < FRESH_FOR)
            .map(|(_, answer)| answer.clone())
    }

    pub(crate) fn keep(&self, answer: &MemoryStatsResponse) {
        self.answers
            .lock()
            .insert(key(), (Instant::now(), answer.clone()));
    }
}
