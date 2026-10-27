mod rows;
mod write;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::topics::MergePlan;
use kernel::topics::PlacedSession;
use kernel::topics::Placement;
use kernel::topics::PlacementOutcome;
use kernel::topics::Redirect;
use kernel::topics::Suggestion;
use kernel::topics::Topic;
use kernel::topics::TopicChanges;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicKey;
use kernel::topics::TopicListing;
use kernel::topics::TopicStore;
use kernel::topics::by_activity;
use kernel::topics::check_parent;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use wire::session_id::SessionId;

use crate::SqliteStore;
use crate::error::SqliteError;
use rows::failed;
use rows::not_found;

impl SqliteStore {
    async fn topics<T, F>(&self, operation: &'static str, work: F) -> Result<T, TopicError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, TopicError> + Send + 'static,
    {
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(SqliteError::query(operation))?;
            let done = work(&tx);
            if done.is_ok() {
                tx.commit().map_err(SqliteError::query(operation))?;
            }
            Ok(done)
        })
        .await
        .map_err(|error| TopicError::Unavailable(Box::new(error)))?
    }
}

fn stored(conn: &Connection, id: TopicId) -> Result<Topic, TopicError> {
    rows::by_id(conn, id)?.ok_or_else(|| not_found(id))
}

#[async_trait]
impl TopicStore for SqliteStore {
    async fn create(&self, topic: Topic) -> Result<Topic, TopicError> {
        self.topics("topic create", move |conn| {
            if let Some(parent) = topic.parent {
                let parent = rows::by_id(conn, parent)?.ok_or_else(|| not_found(parent))?;
                check_parent(topic.id, false, &parent)?;
            }
            write::claim_names(conn, &topic)?;
            write::insert(conn, &topic)?;
            write::aliases(conn, &topic)?;
            stored(conn, topic.id)
        })
        .await
    }

    async fn update(
        &self,
        id: TopicId,
        changes: TopicChanges,
        now: DateTime<Utc>,
    ) -> Result<Topic, TopicError> {
        self.topics("topic update", move |conn| {
            let current = rows::by_id(conn, id)?.ok_or_else(|| not_found(id))?;
            let old_slug = current.slug.clone();
            let updated = current.apply(changes, now)?;
            if let Some(parent) = updated.parent {
                let parent = rows::by_id(conn, parent)?.ok_or_else(|| not_found(parent))?;
                check_parent(id, !rows::subtopics(conn, id)?.is_empty(), &parent)?;
            }
            write::claim_names(conn, &updated)?;
            write::row(conn, &updated)?;
            write::aliases(conn, &updated)?;
            if updated.slug != old_slug {
                write::rename_redirect(conn, &old_slug, &updated)?;
            }
            stored(conn, id)
        })
        .await
    }

    async fn merge(
        &self,
        source: TopicId,
        into: TopicId,
        now: DateTime<Utc>,
    ) -> Result<Topic, TopicError> {
        self.topics("topic merge", move |conn| {
            let from = rows::by_id(conn, source)?.ok_or_else(|| not_found(source))?;
            let target = rows::by_id(conn, into)?.ok_or_else(|| not_found(into))?;
            let plan = MergePlan::new(&from, &target, &rows::subtopics(conn, source)?)?;
            let merged = plan.merged(target, now);
            write::fold_into(conn, &plan, &merged)?;
            stored(conn, into)
        })
        .await
    }

    async fn find(&self, key: &TopicKey) -> Result<Option<Topic>, TopicError> {
        let key = key.clone();
        self.topics("topic read", move |conn| match &key {
            TopicKey::Id(id) => rows::by_id(conn, *id),
            TopicKey::Slug(slug) => rows::by_slug(conn, slug),
        })
        .await
    }

    async fn list(&self) -> Result<Vec<TopicListing>, TopicError> {
        let mut listings = self.topics("topic list", rows::listings).await?;
        listings.sort_by(by_activity);
        Ok(listings)
    }

    async fn redirects(&self) -> Result<Vec<Redirect>, TopicError> {
        self.topics("topic redirects", rows::redirects).await
    }

    async fn place(&self, placement: Placement) -> Result<PlacementOutcome, TopicError> {
        self.topics("session placement", move |conn| {
            if rows::by_id(conn, placement.topic)?.is_none() {
                return Err(not_found(placement.topic));
            }
            let existing = rows::placement_of(conn, &placement.session)?;
            match placement.admit(existing) {
                PlacementOutcome::Placed(placed) => {
                    write::placement(conn, &placed)?;
                    rows::placement_of(conn, &placed.session)?
                        .map(PlacementOutcome::Placed)
                        .ok_or_else(|| not_found(placed.topic))
                }
                kept @ PlacementOutcome::Kept(_) => Ok(kept),
            }
        })
        .await
    }

    async fn unplace(&self, session: &SessionId) -> Result<Option<Placement>, TopicError> {
        let session = session.clone();
        self.topics("session unplace", move |conn| {
            let existing = rows::placement_of(conn, &session)?;
            conn.execute(
                "DELETE FROM rh_session_placements WHERE session_id = ?1",
                params![session.as_str()],
            )
            .map_err(failed("session unplace"))?;
            Ok(existing)
        })
        .await
    }

    async fn placement(&self, session: &SessionId) -> Result<Option<Placement>, TopicError> {
        let session = session.clone();
        self.topics("session placement read", move |conn| {
            rows::placement_of(conn, &session)
        })
        .await
    }

    async fn sessions(&self, topic: TopicId) -> Result<Vec<PlacedSession>, TopicError> {
        self.topics("topic sessions", move |conn| rows::placed_in(conn, topic))
            .await
    }

    async fn suggest(&self, suggestion: Suggestion) -> Result<(), TopicError> {
        self.topics("session suggestion", move |conn| {
            if rows::placement_of(conn, &suggestion.session)?.is_none() {
                write::suggestion(conn, &suggestion)?;
            }
            Ok(())
        })
        .await
    }

    async fn suggestion(&self, session: &SessionId) -> Result<Option<Suggestion>, TopicError> {
        let session = session.clone();
        self.topics("session suggestion read", move |conn| {
            rows::suggestion_of(conn, &session)
        })
        .await
    }

    async fn page_slug(&self, topic: TopicId) -> Result<Option<String>, TopicError> {
        self.topics("topic page", move |conn| {
            conn.query_row(
                "SELECT page_slug FROM rh_topic_pages WHERE topic_id = ?1",
                params![topic.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed("topic page"))
        })
        .await
    }
}
