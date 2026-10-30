use kernel::topics::TopicId;
use kernel::topics::TopicKind;
use kernel::wiki::WikiFilter;
use kernel::wiki::builder::ExtractWiki;

use crate::topics::Outcome;
use crate::topics::new_topic;
use crate::topics::same;
use crate::wiki_routes::RouteStores;
use crate::wiki_routes::SESSION_KEY;
use crate::wiki_routes::SESSION_TITLE;
use crate::wiki_routes::host;

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
