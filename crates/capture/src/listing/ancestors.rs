use std::collections::HashMap;
use std::collections::HashSet;

use kernel::transcript::StoreSession;
use wire::harness::HarnessId;

const MAX_DEPTH: usize = 8;

pub fn with_ancestors(ranked: Vec<StoreSession>, limit: usize) -> Vec<StoreSession> {
    let cap = if limit > 0 { limit } else { ranked.len() };
    if cap >= ranked.len() {
        return ranked;
    }
    let pool: HashMap<(HarnessId, &str), &StoreSession> = ranked
        .iter()
        .map(|row| ((row.harness, row.id.as_str()), row))
        .collect();
    let kept: Vec<&StoreSession> = ranked.iter().take(cap).collect();
    let mut included: HashSet<(HarnessId, &str)> = kept
        .iter()
        .map(|row| (row.harness, row.id.as_str()))
        .collect();
    let mut extras: Vec<&StoreSession> = Vec::new();
    for row in &kept {
        let mut parent = row.parent_session_id.as_deref();
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(id) = parent {
            if seen.contains(id) || seen.len() >= MAX_DEPTH {
                break;
            }
            seen.insert(id);
            let Some(found) = pool.get(&(row.harness, id)) else {
                break;
            };
            if included.insert((found.harness, found.id.as_str())) {
                extras.push(found);
            }
            parent = found.parent_session_id.as_deref();
        }
    }
    kept.into_iter().chain(extras).cloned().collect()
}
