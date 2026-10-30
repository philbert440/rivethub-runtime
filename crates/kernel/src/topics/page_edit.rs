use chrono::DateTime;
use chrono::Utc;
use wire::gateway::wiki::WikiHistoryEntry;
use wire::gateway::wiki::WikiSlug;
use wire::gateway::wiki::WikiSourceKind;
use wire::wiki::PageSource;
use wire::wiki::WikiPage;

use super::Topic;
use super::TopicPageError;
use super::TopicPageStore;
use super::TopicStore;
use super::page_owner;
use super::root_page;
use crate::wiki::builder::WikiStoreError;

const USER_EDIT: &str = "Edited in Memory";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageEdit {
    pub current_state: Option<String>,
    pub article: Option<String>,
}

impl PageEdit {
    pub fn apply_to(&self, page: &mut WikiPage, now: DateTime<Utc>) {
        if let Some(text) = &self.current_state {
            text.trim().clone_into(&mut page.current_state);
        }
        if let Some(text) = &self.article {
            text.trim().clone_into(&mut page.article);
        }
        let entry = WikiHistoryEntry {
            date: now.date_naive(),
            title: USER_EDIT.to_owned(),
            body: "Edited by hand in RivetHub's Memory page.".to_owned(),
        };
        if !page.history.contains(&entry) {
            page.history.insert(0, entry);
        }
        if !page
            .meta
            .sources
            .iter()
            .any(|source| source.kind == WikiSourceKind::User)
        {
            page.meta.sources.push(PageSource {
                kind: WikiSourceKind::User,
                ids: Vec::new(),
                conversation_id: None,
                extra: wire::Extra::default(),
            });
        }
    }

    #[must_use]
    pub fn commit_message(slug: &WikiSlug) -> String {
        format!("wiki({}): {USER_EDIT}\n\nSource: user", slug.as_ref())
    }
}

pub async fn edit_topic_page(
    topics: &dyn TopicStore,
    pages: &dyn TopicPageStore,
    topic: &Topic,
    edit: &PageEdit,
    now: DateTime<Utc>,
) -> Result<Option<WikiSlug>, TopicPageError> {
    let Some(owner) = page_owner(topics, topic).await? else {
        return Ok(None);
    };
    let slug = root_page(topics, pages, &owner, now).await?;
    if !pages.edit_page(&slug, edit, now).await? {
        return Err(WikiStoreError::TopicMissing(slug.as_ref().to_owned()).into());
    }
    Ok(Some(slug))
}
