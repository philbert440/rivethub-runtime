use capture::listing::StoreSessionLister;
use capture::listing::store_roots;
use config::Environment;
use kernel::cleanup::Snapshot;
use kernel::filing::Filable;
use kernel::transcript::session_parent;
use wire::session_id::SessionId;

use crate::Error;

pub(super) async fn enrich(snapshot: &mut Snapshot, env: &Environment) -> Result<usize, Error> {
    let lineage = StoreSessionLister::new(store_roots(env));
    let mut children = 0;
    for conversation in &mut snapshot.conversations {
        let Filable::Session(session) = Filable::of(&conversation.head.head()) else {
            continue;
        };
        if let Some(parent) = session_parent(&lineage, &session).await? {
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
