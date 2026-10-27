use kernel::topics::Alias;
use kernel::topics::MergePlan;
use kernel::topics::Placement;
use kernel::topics::Root;
use kernel::topics::Slug;
use kernel::topics::Suggestion;
use kernel::topics::Topic;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use sqlx::PgConnection;
use uuid::Uuid;
use wire::session_id::SessionId;

use super::rows::failed;

const SLUG_INDEX: &str = "ux_rh_topics_slug";
const ALIAS_INDEX: &str = "ux_rh_topic_aliases_key";

fn written(operation: &'static str, topic: &Topic) -> impl FnOnce(sqlx::Error) -> TopicError {
    move |source| {
        let violated = |index: &str| {
            source.as_database_error().is_some_and(|error| {
                error.is_unique_violation() && error.constraint() == Some(index)
            })
        };
        if violated(SLUG_INDEX) {
            TopicError::SlugTaken {
                slug: topic.slug.clone(),
            }
        } else if violated(ALIAS_INDEX) {
            TopicError::AliasTaken {
                alias: topic
                    .aliases
                    .iter()
                    .map(Alias::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        } else {
            failed(operation)(source)
        }
    }
}

pub(super) async fn claim_names(conn: &mut PgConnection, topic: &Topic) -> Result<(), TopicError> {
    let slug_taken = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM rh_topics WHERE slug = $1 AND id <> $2) AS "taken!""#,
        topic.slug.as_str(),
        topic.id.uuid()
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(failed("topic slug check"))?;
    if slug_taken {
        return Err(TopicError::SlugTaken {
            slug: topic.slug.clone(),
        });
    }
    let keys: Vec<String> = topic.aliases.iter().map(Alias::key).collect();
    let owned = sqlx::query_scalar!(
        "SELECT alias FROM rh_topic_aliases
          WHERE alias_key = ANY($1::text[]) AND topic_id <> $2
          ORDER BY alias_key LIMIT 1",
        &keys,
        topic.id.uuid()
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(failed("topic alias check"))?;
    match owned {
        Some(alias) => Err(TopicError::AliasTaken { alias }),
        None => Ok(()),
    }
}

pub(super) async fn insert(conn: &mut PgConnection, topic: &Topic) -> Result<(), TopicError> {
    let directory = topic.directory.as_ref();
    sqlx::query!(
        "INSERT INTO rh_topics (id, slug, name, parent_id, directory_kind, directory_node,
                                directory_path, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        topic.id.uuid(),
        topic.slug.as_str(),
        topic.name.as_str(),
        topic.parent.map(TopicId::uuid),
        directory.map(|root| root.kind().to_string()),
        directory.and_then(Root::node),
        directory.map(Root::path),
        topic.created_at,
        topic.updated_at
    )
    .execute(&mut *conn)
    .await
    .map_err(written("topic create", topic))?;
    Ok(())
}

pub(super) async fn row(conn: &mut PgConnection, topic: &Topic) -> Result<(), TopicError> {
    let directory = topic.directory.as_ref();
    sqlx::query!(
        "UPDATE rh_topics
            SET slug = $2, name = $3, parent_id = $4, directory_kind = $5, directory_node = $6,
                directory_path = $7, updated_at = $8
          WHERE id = $1",
        topic.id.uuid(),
        topic.slug.as_str(),
        topic.name.as_str(),
        topic.parent.map(TopicId::uuid),
        directory.map(|root| root.kind().to_string()),
        directory.and_then(Root::node),
        directory.map(Root::path),
        topic.updated_at
    )
    .execute(&mut *conn)
    .await
    .map_err(written("topic update", topic))?;
    Ok(())
}

pub(super) async fn aliases(conn: &mut PgConnection, topic: &Topic) -> Result<(), TopicError> {
    sqlx::query!(
        "DELETE FROM rh_topic_aliases WHERE topic_id = $1",
        topic.id.uuid()
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic aliases"))?;
    let texts: Vec<String> = topic
        .aliases
        .iter()
        .map(|alias| alias.as_str().to_owned())
        .collect();
    let keys: Vec<String> = topic.aliases.iter().map(Alias::key).collect();
    sqlx::query!(
        "INSERT INTO rh_topic_aliases (topic_id, alias, alias_key)
         SELECT $1, alias, alias_key FROM unnest($2::text[], $3::text[]) AS a(alias, alias_key)",
        topic.id.uuid(),
        &texts,
        &keys
    )
    .execute(&mut *conn)
    .await
    .map_err(written("topic aliases", topic))?;
    Ok(())
}

pub(super) async fn subtopics(
    conn: &mut PgConnection,
    id: TopicId,
) -> Result<Vec<TopicId>, TopicError> {
    let ids = sqlx::query_scalar!(
        "SELECT id FROM rh_topics WHERE parent_id = $1 ORDER BY id FOR UPDATE",
        id.uuid()
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(failed("sub-topic read"))?;
    Ok(ids.into_iter().map(TopicId::from_uuid).collect())
}

pub(super) async fn fold_into(
    conn: &mut PgConnection,
    plan: &MergePlan,
    merged: &Topic,
) -> Result<(), TopicError> {
    let source = plan.source.uuid();
    let into = plan.into.uuid();
    sqlx::query!(
        "UPDATE rh_session_placements SET topic_id = $2 WHERE topic_id = $1",
        source,
        into
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic merge placements"))?;
    sqlx::query!(
        "UPDATE rh_topic_redirects SET topic_id = $2 WHERE topic_id = $1",
        source,
        into
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic merge redirects"))?;
    sqlx::query!(
        "UPDATE rh_session_suggestions SET topic_id = $2 WHERE topic_id = $1",
        source,
        into
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic merge suggestions"))?;
    sqlx::query!("DELETE FROM rh_topic_aliases WHERE topic_id = $1", source)
        .execute(&mut *conn)
        .await
        .map_err(failed("topic merge aliases"))?;
    let moved: Vec<Uuid> = plan.subtopics.iter().map(|id| id.uuid()).collect();
    sqlx::query!(
        "UPDATE rh_topics SET parent_id = $2, updated_at = $3 WHERE id = ANY($1::uuid[])",
        &moved,
        into,
        merged.updated_at
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic merge sub-topics"))?;
    row(conn, merged).await?;
    aliases(conn, merged).await?;
    sqlx::query!("DELETE FROM rh_topics WHERE id = $1", source)
        .execute(&mut *conn)
        .await
        .map_err(failed("topic merge delete"))?;
    sqlx::query!(
        "INSERT INTO rh_topic_redirects (slug, topic_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT (slug) DO UPDATE SET topic_id = EXCLUDED.topic_id,
                                          created_at = EXCLUDED.created_at",
        plan.redirect.slug.as_str(),
        plan.redirect.topic.uuid(),
        merged.updated_at
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic merge redirect"))?;
    Ok(())
}

pub(super) async fn rename_redirect(
    conn: &mut PgConnection,
    old: &Slug,
    renamed: &Topic,
) -> Result<(), TopicError> {
    sqlx::query!(
        "DELETE FROM rh_topic_redirects WHERE slug = $1",
        renamed.slug.as_str()
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic rename redirect"))?;
    sqlx::query!(
        "INSERT INTO rh_topic_redirects (slug, topic_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT (slug) DO UPDATE SET topic_id = EXCLUDED.topic_id,
                                          created_at = EXCLUDED.created_at",
        old.as_str(),
        renamed.id.uuid(),
        renamed.updated_at
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic rename redirect"))?;
    Ok(())
}

pub(super) async fn lock_session(
    conn: &mut PgConnection,
    session: &SessionId,
) -> Result<(), TopicError> {
    sqlx::query_scalar!(
        r#"SELECT true AS "locked!" FROM (SELECT pg_advisory_xact_lock(hashtextextended($1, 0))) AS held"#,
        session.as_str()
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(failed("session placement lock"))?;
    Ok(())
}

pub(super) async fn topic_exists(conn: &mut PgConnection, id: TopicId) -> Result<bool, TopicError> {
    let found = sqlx::query_scalar!(
        "SELECT id FROM rh_topics WHERE id = $1 FOR SHARE",
        id.uuid()
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(failed("session placement topic"))?;
    Ok(found.is_some())
}

pub(super) async fn placement(
    conn: &mut PgConnection,
    placement: &Placement,
) -> Result<(), TopicError> {
    sqlx::query!(
        "INSERT INTO rh_session_placements (session_id, topic_id, placed_by, placed_at)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (session_id) DO UPDATE SET topic_id = EXCLUDED.topic_id,
                                                placed_by = EXCLUDED.placed_by,
                                                placed_at = EXCLUDED.placed_at",
        placement.session.as_str(),
        placement.topic.uuid(),
        placement.by.to_string(),
        placement.at
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("session placement write"))?;
    sqlx::query!(
        "DELETE FROM rh_session_suggestions WHERE session_id = $1",
        placement.session.as_str()
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("session suggestion clear"))?;
    Ok(())
}

pub(super) async fn suggestion(
    conn: &mut PgConnection,
    suggestion: &Suggestion,
) -> Result<(), TopicError> {
    sqlx::query!(
        "INSERT INTO rh_session_suggestions (session_id, name, topic_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (session_id) DO UPDATE SET name = EXCLUDED.name,
                                                topic_id = EXCLUDED.topic_id",
        suggestion.session.as_str(),
        suggestion.name.as_str(),
        suggestion.topic.map(TopicId::uuid)
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("session suggestion write"))?;
    Ok(())
}
