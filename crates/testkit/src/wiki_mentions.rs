use kernel::topics::NewTopic;
use kernel::topics::Slug;
use kernel::topics::Topic;
use kernel::topics::TopicId;
use kernel::topics::TopicKind;
use kernel::topics::TopicName;
use kernel::wiki::WikiFilter;
use kernel::wiki::builder::ExtractWiki;
use kernel::wiki::builder::HeldEntity;

use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::new_topic;
use crate::topics::same;
use crate::wiki_routes::RouteStores;
use crate::wiki_routes::SESSION_KEY;
use crate::wiki_routes::SESSION_TITLE;
use crate::wiki_routes::host;

pub const EARLIER_KEY: &str = "claude-code:00000000-0000-4000-8000-000000000043";

type Found<T> = Result<T, Box<dyn std::error::Error>>;

pub async fn an_entity_page_names_the_sessions_that_cite_it(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
) -> Outcome {
    let acmekit = stores
        .pages
        .create(new_topic("AcmeKit", &[], None)?)
        .await?;
    stores.place(&acmekit).await?;
    let host = host("The build box runs the nightly AcmeKit builds.")?;
    stores.mentioned_elsewhere(job, &host).await?;
    stores.route(job, vec![host]).await?;
    let entity = stores
        .named(TopicKind::Entity, "AcmeKit build box")
        .await?
        .ok_or("the second mention creates the entity")?;
    let slug = stores.pages.slug(&entity).await?;
    let cited: Vec<_> = stores
        .pages
        .shown(slug.as_ref())
        .await?
        .citations
        .into_iter()
        .map(|citation| {
            (
                citation.conversation_id,
                citation
                    .session_id
                    .map(|session| session.as_str().to_owned()),
                citation.title,
                citation.topic_id,
            )
        })
        .collect();
    same(
        "cited by",
        cited,
        vec![(
            job.conversation_id.map(|id| id.to_string()),
            Some(SESSION_KEY.to_owned()),
            Some(SESSION_TITLE.to_owned()),
            Some(acmekit.id.to_string()),
        )],
    )?;
    same(
        "the topic mentions the entity",
        mentioned(&stores, acmekit.id).await?,
        vec![(entity.id, "AcmeKit build box".to_owned())],
    )?;
    let elsewhere = stores
        .pages
        .create(new_topic("Billing", &[], None)?)
        .await?;
    same(
        "another topic mentions nothing",
        mentioned(&stores, elsewhere.id).await?,
        Vec::new(),
    )?;
    same(
        "owners",
        owners(&stores).await?,
        vec![
            ("acmekit".to_owned(), Some(acmekit.id.to_string())),
            (slug.as_ref().to_owned(), Some(entity.id.to_string())),
            ("billing".to_owned(), Some(elsewhere.id.to_string())),
        ],
    )?;
    Ok(())
}

pub async fn a_mention_held_before_its_entity_existed_still_lands(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
    earlier: &ExtractWiki,
) -> Outcome {
    let hosts = stores
        .named(TopicKind::Type, "Hosts")
        .await?
        .ok_or("the Hosts type is seeded")?;
    let key = Slug::fold("AcmeKit build box")?;
    stores
        .wiki
        .hold_entity(HeldEntity {
            key: key.clone(),
            source: EARLIER_KEY.to_owned(),
            summary_id: earlier.summary_id,
            patch: host("The build box sits in the lab rack.")?,
            at: at(2),
        })
        .await?;
    let entity = stores
        .pages
        .create(Topic::create(
            TopicId::random(),
            NewTopic {
                kind: TopicKind::Entity,
                name: TopicName::new("AcmeKit build box")?,
                aliases: Vec::new(),
                parent: Some(hosts.id),
                directory: None,
            },
            at(0),
        )?)
        .await?;
    stores
        .route(job, vec![host("The build box runs the nightly builds.")?])
        .await?;
    let slug = stores.pages.slug(&entity).await?;
    let mut cited: Vec<String> = stores
        .pages
        .shown(slug.as_ref())
        .await?
        .citations
        .into_iter()
        .map(|citation| citation.summary_id)
        .collect();
    cited.sort();
    let mut both = vec![earlier.summary_id.to_string(), job.summary_id.to_string()];
    both.sort();
    same("the held mention is written and cited", cited, both)?;
    same(
        "the written mention is no longer held",
        stores.wiki.held_entities(&key).await?,
        Vec::new(),
    )?;
    Ok(())
}

async fn mentioned(stores: &RouteStores<'_>, topic: TopicId) -> Found<Vec<(TopicId, String)>> {
    Ok(stores
        .filing
        .mentions(&[topic], 10)
        .await?
        .into_iter()
        .map(|mention| (mention.entity, mention.name.as_str().to_owned()))
        .collect())
}

async fn owners(stores: &RouteStores<'_>) -> Found<Vec<(String, Option<String>)>> {
    let listed = stores
        .pages
        .wiki
        .wiki_topics(WikiFilter {
            query: None,
            tag: None,
            limit: 50,
            offset: 0,
        })
        .await?;
    let mut owners: Vec<(String, Option<String>)> = listed
        .topics
        .into_iter()
        .map(|topic| (topic.slug, topic.topic_id))
        .collect();
    owners.sort();
    Ok(owners)
}
