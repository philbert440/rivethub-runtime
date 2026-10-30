use capture::listing::StoreSessionLister;
use capture::listing::store_roots;
use config::Environment;
use kernel::cleanup::Snapshot;
use kernel::filing::Filable;
use kernel::transcript::SessionLister as _;
use wire::session_id::SessionId;

use crate::Error;

const SUBAGENTS: &str = "/subagents/";

pub(super) async fn enrich(snapshot: &mut Snapshot, env: &Environment) -> Result<usize, Error> {
    let lineage = StoreSessionLister::new(store_roots(env));
    let mut children = 0;
    for conversation in &mut snapshot.conversations {
        let Filable::Session(session) = Filable::of(&conversation.head.head()) else {
            continue;
        };
        let parent = match session.native().split_once(SUBAGENTS) {
            Some((parent, _)) => Some(parent.to_owned()),
            None => lineage
                .describe(session.harness(), session.native(), None)
                .await?
                .and_then(|row| row.parent_session_id),
        };
        if let Some(parent) = parent {
            conversation.parent_session = Some(
                SessionId::new(session.harness(), &parent)
                    .as_str()
                    .to_owned(),
            );
            children += 1;
        }
    }
    Ok(children)
}
