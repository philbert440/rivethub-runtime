use std::collections::BTreeMap;
use std::str::FromStr as _;

use kernel::filing::conversation_keys;
use kernel::tidy::Proposal;
use kernel::tidy::TidyError;
use wire::session_id::SessionId;

use super::failed;
use crate::PostgresMemory;

const TITLES_SQL: &str = "select k.key, c.title, c.updated_at
      from unnest($1::text[]) k(key) join rh_conversations c on c.session_key = k.key
     where c.title is not null
    union all
    select k.key, c.title, c.updated_at
      from unnest($1::text[]) k(key) join rh_conversations c on c.id::text = k.key
     where c.title is not null
    order by updated_at";

#[derive(Debug, sqlx::FromRow)]
struct KeyTitle {
    key: String,
    title: String,
}

impl PostgresMemory {
    pub(super) async fn current_titles(&self, proposals: &mut [Proposal]) -> Result<(), TidyError> {
        let mut owners: BTreeMap<String, String> = BTreeMap::new();
        for sample in proposals
            .iter()
            .flat_map(|proposal| &proposal.evidence.samples)
        {
            owners.insert(sample.id.clone(), sample.id.clone());
            if let Ok(session) = SessionId::from_str(&sample.id) {
                for key in conversation_keys(&session) {
                    owners.insert(key, sample.id.clone());
                }
            }
        }
        if owners.is_empty() {
            return Ok(());
        }
        let keys: Vec<&str> = owners.keys().map(String::as_str).collect();
        let rows = sqlx::query_as::<_, KeyTitle>(TITLES_SQL)
            .bind(&keys)
            .fetch_all(&self.pool)
            .await
            .map_err(failed("tidy sample titles"))?;
        let titles: BTreeMap<&str, String> = rows
            .into_iter()
            .filter_map(|row| Some((owners.get(&row.key)?.as_str(), row.title)))
            .collect();
        for sample in proposals
            .iter_mut()
            .flat_map(|proposal| &mut proposal.evidence.samples)
        {
            if let Some(title) = titles.get(sample.id.as_str()) {
                sample.title = Some(title.clone());
            }
        }
        Ok(())
    }
}
