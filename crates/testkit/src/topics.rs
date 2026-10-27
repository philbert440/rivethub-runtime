use std::fmt::Debug;
use std::str::FromStr;

use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use kernel::topics::Alias;
use kernel::topics::Change;
use kernel::topics::NewTopic;
use kernel::topics::Placement;
use kernel::topics::PlacementOutcome;
use kernel::topics::PlacementSource;
use kernel::topics::Root;
use kernel::topics::Slug;
use kernel::topics::Suggestion;
use kernel::topics::Topic;
use kernel::topics::TopicChanges;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicKey;
use kernel::topics::TopicName;
use kernel::topics::TopicQuery;
use kernel::topics::TopicStore;
use wire::session_id::SessionId;

pub type Outcome = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, thiserror::Error)]
#[error("{what}: expected {expected}, got {actual}")]
pub struct Mismatch {
    what: &'static str,
    expected: String,
    actual: String,
}

pub(crate) fn same<T: PartialEq + Debug>(
    what: &'static str,
    actual: T,
    expected: T,
) -> Result<(), Mismatch> {
    match (actual, expected) {
        (actual, expected) if actual == expected => Ok(()),
        (actual, expected) => Err(Mismatch {
            what,
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        }),
    }
}

#[must_use]
pub fn at(minute: u32) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + TimeDelta::seconds(1_790_000_000 + i64::from(minute) * 60)
}

pub fn session(native: &str) -> Result<SessionId, wire::session_id::SessionIdError> {
    SessionId::from_str(&format!("claude-code:{native}"))
}

pub fn new_topic(
    name: &str,
    aliases: &[&str],
    parent: Option<TopicId>,
) -> Result<Topic, TopicError> {
    Topic::create(
        TopicId::random(),
        NewTopic {
            name: TopicName::new(name)?,
            aliases: aliases
                .iter()
                .map(|alias| Alias::new(alias))
                .collect::<Result<_, _>>()?,
            parent,
            directory: None,
        },
        at(0),
    )
}

fn alias_texts(topic: &Topic) -> Vec<&str> {
    topic.aliases.iter().map(Alias::as_str).collect()
}

pub async fn topics_are_created_found_and_renamed(store: &dyn TopicStore) -> Outcome {
    let mut topic = new_topic("RivetHub Desktop", &["Tauri", "desktop app"], None)?;
    topic.directory = Some(Root::Local {
        node: "node-a".into(),
        path: "/home/user/acmekit".into(),
    });
    topic.created_at = at(0) + TimeDelta::nanoseconds(123_456_789);
    topic.updated_at = topic.created_at;
    let created = store.create(topic.clone()).await?;
    same("created", created.name.as_str(), topic.name.as_str())?;
    same(
        "created reads back unchanged",
        store.find(&TopicKey::Id(topic.id)).await?.as_ref(),
        Some(&created),
    )?;
    let topic = created;
    same(
        "by id",
        store.find(&TopicKey::Id(topic.id)).await?.as_ref(),
        Some(&topic),
    )?;
    same(
        "by slug",
        store
            .find(&TopicKey::parse("rivethub desktop")?)
            .await?
            .as_ref(),
        Some(&topic),
    )?;
    let renamed = store
        .update(
            topic.id,
            TopicChanges {
                name: Some(TopicName::new("Desktop Client")?),
                aliases: Some(vec![Alias::new("electron")?]),
                directory: Change::Set(Root::Shared {
                    path: "/srv/shared/acmekit".into(),
                }),
                ..TopicChanges::default()
            },
            at(3),
        )
        .await?;
    same("renamed slug", renamed.slug.as_str(), "desktop-client")?;
    same("replaced aliases", alias_texts(&renamed), vec!["electron"])?;
    same(
        "stored rename",
        store.find(&TopicKey::Id(topic.id)).await?.as_ref(),
        Some(&renamed),
    )?;
    same(
        "old slug still finds it",
        store
            .find(&TopicKey::parse("rivethub-desktop")?)
            .await?
            .map(|found| found.id),
        Some(topic.id),
    )?;
    let missing = store.find(&TopicKey::parse("no-such-topic")?).await?;
    same("missing", missing, None)?;
    let listed = store.list().await?;
    same("listed", listed.len(), 1)?;
    Ok(())
}

pub async fn slugs_and_aliases_are_unique_across_topics(store: &dyn TopicStore) -> Outcome {
    let first = store.create(new_topic("Deploy", &["Ship"], None)?).await?;
    let clash = store.create(new_topic("deploy!", &[], None)?).await;
    if !matches!(clash, Err(TopicError::SlugTaken { .. })) {
        return Err(format!("a second deploy slug must be refused: {clash:?}").into());
    }
    let alias = store.create(new_topic("Release", &["SHIP"], None)?).await;
    if !matches!(alias, Err(TopicError::AliasTaken { ref alias }) if alias == "Ship") {
        return Err(format!("a case-folded alias clash must be refused: {alias:?}").into());
    }
    let other = store
        .create(new_topic("Release", &["train"], None)?)
        .await?;
    let renamed = store
        .update(
            other.id,
            TopicChanges {
                name: Some(TopicName::new("Deploy")?),
                ..TopicChanges::default()
            },
            at(1),
        )
        .await;
    if !matches!(renamed, Err(TopicError::SlugTaken { .. })) {
        return Err(format!("a rename onto a taken slug must be refused: {renamed:?}").into());
    }
    same("first kept", store.list().await?.len(), 2)?;
    same(
        "first still owns its alias",
        store
            .find(&TopicKey::Id(first.id))
            .await?
            .map(|topic| alias_texts(&topic).join(",")),
        Some("Ship".to_owned()),
    )?;
    Ok(())
}

pub async fn sub_topics_stay_one_level_deep(store: &dyn TopicStore) -> Outcome {
    let parent = store.create(new_topic("Runtime", &[], None)?).await?;
    let child = store
        .create(new_topic("Topics", &[], Some(parent.id))?)
        .await?;
    let grandchild = store.create(new_topic("Merge", &[], Some(child.id))?).await;
    if !matches!(grandchild, Err(TopicError::ParentIsSubtopic { .. })) {
        return Err(format!("a third level must be refused: {grandchild:?}").into());
    }
    let orphan = store
        .create(new_topic("Orphan", &[], Some(TopicId::random()))?)
        .await;
    if !matches!(orphan, Err(TopicError::NotFound { .. })) {
        return Err(format!("an unknown parent must be refused: {orphan:?}").into());
    }
    let other = store.create(new_topic("Gateway", &[], None)?).await?;
    let nested = store
        .update(
            parent.id,
            TopicChanges {
                parent: Change::Set(other.id),
                ..TopicChanges::default()
            },
            at(1),
        )
        .await;
    if !matches!(nested, Err(TopicError::HasSubtopics)) {
        return Err(format!("a parent cannot become a sub-topic: {nested:?}").into());
    }
    let cleared = store
        .update(
            child.id,
            TopicChanges {
                parent: Change::Clear,
                ..TopicChanges::default()
            },
            at(2),
        )
        .await?;
    same("cleared parent", cleared.parent, None)?;
    Ok(())
}

pub async fn placements_keep_your_choice(store: &dyn TopicStore) -> Outcome {
    let mine = store.create(new_topic("Mine", &[], None)?).await?;
    let suggested = store.create(new_topic("Suggested", &[], None)?).await?;
    let session = session("00000000-0000-4000-8000-000000000001")?;
    let placed = |topic: TopicId, by: PlacementSource, minute: u32| Placement {
        session: session.clone(),
        topic,
        by,
        at: at(minute),
    };
    same("forming", store.placement(&session).await?, None)?;
    let first = placed(suggested.id, PlacementSource::Categorizer, 1);
    same(
        "categorizer places a forming session",
        store.place(first.clone()).await?,
        PlacementOutcome::Placed(first),
    )?;
    let yours = placed(mine.id, PlacementSource::You, 2);
    same(
        "you move it",
        store.place(yours.clone()).await?,
        PlacementOutcome::Placed(yours.clone()),
    )?;
    same(
        "categorizer cannot override you",
        store
            .place(placed(suggested.id, PlacementSource::Categorizer, 3))
            .await?,
        PlacementOutcome::Kept(yours.clone()),
    )?;
    same(
        "stored placement",
        store.placement(&session).await?,
        Some(yours.clone()),
    )?;
    let sessions = store.sessions(mine.id).await?;
    same(
        "sessions in topic",
        sessions
            .iter()
            .map(|placed| &placed.placement)
            .collect::<Vec<_>>(),
        vec![&yours],
    )?;
    let unknown = store
        .place(placed(TopicId::random(), PlacementSource::You, 4))
        .await;
    if !matches!(unknown, Err(TopicError::NotFound { .. })) {
        return Err(format!("an unknown topic cannot hold sessions: {unknown:?}").into());
    }
    same("unplaced", store.unplace(&session).await?, Some(yours))?;
    same("forming again", store.placement(&session).await?, None)?;
    same("unplace twice", store.unplace(&session).await?, None)?;
    Ok(())
}

pub async fn merges_move_placements_aliases_and_slugs(store: &dyn TopicStore) -> Outcome {
    let source = store
        .create(new_topic("Desktop App", &["electron", "Tauri"], None)?)
        .await?;
    let into = store
        .create(new_topic("RivetHub Desktop", &["rh-desktop"], None)?)
        .await?;
    let child = store
        .create(new_topic("Desktop Menus", &[], Some(source.id))?)
        .await?;
    let session = session("00000000-0000-4000-8000-000000000002")?;
    let placement = Placement {
        session: session.clone(),
        topic: source.id,
        by: PlacementSource::Categorizer,
        at: at(1),
    };
    store.place(placement.clone()).await?;
    let merged = store.merge(source.id, into.id, at(5)).await?;
    same(
        "merged aliases",
        alias_texts(&merged),
        vec!["electron", "rh-desktop", "Tauri"],
    )?;
    same("merged updated", merged.updated_at, at(5))?;
    same(
        "source gone",
        store.find(&TopicKey::Id(source.id)).await?,
        None,
    )?;
    same(
        "old slug resolves",
        store
            .find(&TopicKey::parse("desktop-app")?)
            .await?
            .map(|topic| topic.id),
        Some(into.id),
    )?;
    same(
        "placement moved",
        store.placement(&session).await?.map(|moved| moved.topic),
        Some(into.id),
    )?;
    same(
        "sub-topic adopted",
        store
            .find(&TopicKey::Id(child.id))
            .await?
            .and_then(|topic| topic.parent),
        Some(into.id),
    )?;
    same(
        "redirect listed",
        store
            .redirects()
            .await?
            .into_iter()
            .map(|redirect| redirect.slug)
            .collect::<Vec<_>>(),
        vec![Slug::fold("desktop-app")?],
    )?;
    let hits = store.search(&TopicQuery::new("desktop a"), 10).await?;
    same(
        "search follows redirects",
        hits.iter().map(|hit| hit.topic.id).collect::<Vec<_>>(),
        vec![into.id],
    )?;
    Ok(())
}

pub async fn redirects_follow_a_second_merge(store: &dyn TopicStore) -> Outcome {
    let first = store.create(new_topic("Desktop App", &[], None)?).await?;
    let second = store
        .create(new_topic("RivetHub Desktop", &[], None)?)
        .await?;
    let last = store.create(new_topic("Clients", &[], None)?).await?;
    store.merge(first.id, second.id, at(5)).await?;
    store.merge(second.id, last.id, at(6)).await?;
    for slug in ["desktop-app", "rivethub-desktop", "clients"] {
        same(
            "redirects follow a second merge",
            store
                .find(&TopicKey::parse(slug)?)
                .await?
                .map(|topic| topic.id),
            Some(last.id),
        )?;
    }
    let refused = store.merge(last.id, last.id, at(7)).await;
    if !matches!(refused, Err(TopicError::SelfMerge)) {
        return Err(format!("a self merge must be refused: {refused:?}").into());
    }
    Ok(())
}

pub async fn suggestions_hold_until_the_session_is_placed(store: &dyn TopicStore) -> Outcome {
    let gateway = store.create(new_topic("Gateway", &[], None)?).await?;
    let mesh = store.create(new_topic("Mesh", &[], None)?).await?;
    let session = session("00000000-0000-4000-8000-000000000021")?;
    same("no suggestion yet", store.suggestion(&session).await?, None)?;
    let unsure = Suggestion {
        session: session.clone(),
        name: TopicName::new("Mesh retries")?,
        topic: Some(mesh.id),
    };
    store.suggest(unsure.clone()).await?;
    same(
        "stored suggestion",
        store.suggestion(&session).await?,
        Some(unsure.clone()),
    )?;
    store.merge(mesh.id, gateway.id, at(2)).await?;
    same(
        "a merge moves the unsure topic",
        store
            .suggestion(&session)
            .await?
            .and_then(|suggestion| suggestion.topic),
        Some(gateway.id),
    )?;
    let renamed = Suggestion {
        name: TopicName::new("Retry budget")?,
        topic: None,
        ..unsure
    };
    store.suggest(renamed.clone()).await?;
    same(
        "a new suggestion replaces the old one",
        store.suggestion(&session).await?,
        Some(renamed.clone()),
    )?;
    store
        .place(Placement {
            session: session.clone(),
            topic: gateway.id,
            by: PlacementSource::Categorizer,
            at: at(3),
        })
        .await?;
    same(
        "a placement clears the suggestion",
        store.suggestion(&session).await?,
        None,
    )?;
    store.suggest(renamed).await?;
    same(
        "a placed session takes no suggestion",
        store.suggestion(&session).await?,
        None,
    )?;
    Ok(())
}
