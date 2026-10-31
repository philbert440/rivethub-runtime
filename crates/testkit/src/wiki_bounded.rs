use async_trait::async_trait;
use kernel::BoxError;
use kernel::topics::TopicKind;
use kernel::wiki::builder::ExtractWiki;
use kernel::wiki::builder::WikiWork;
use kernel::wiki::merge::MergeInput;
use kernel::wiki::merge::Merged;
use kernel::wiki::merge::PageMerger;
use kernel::wiki::page::WikiPatch;
use kernel::wiki::route::TopicTarget;
use kernel::wiki::route::WikiRouter;
use parking_lot::Mutex;

use crate::topics::Outcome;
use crate::topics::new_topic;
use crate::topics::same;
use crate::wiki_routes::RouteStores;
use crate::wiki_routes::host;
use crate::wiki_routes::patch;

type Found<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Default)]
pub struct RecordingMerger {
    pub seen: Mutex<Vec<MergeInput>>,
}

#[async_trait]
impl PageMerger for RecordingMerger {
    async fn merge(&self, input: &MergeInput) -> Result<Merged, BoxError> {
        self.seen.lock().push(input.clone());
        Ok(Merged {
            summary: format!("{} Merged.", input.summary),
            article: format!("{}\n\n{}", input.article, input.material.join("\n\n")),
            history: Some("Merged the new mention.".to_owned()),
        })
    }
}

async fn target(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
) -> Found<(kernel::wiki::builder::WikiSummary, Option<TopicTarget>)> {
    let WikiWork::Pending(summary) = stores.wiki.wiki_work(job).await? else {
        return Err("the summary was already extracted".into());
    };
    let target = stores.router().target(&summary).await?;
    Ok((summary, target))
}

async fn apply(
    router: &WikiRouter<'_>,
    job: &ExtractWiki,
    read: &(kernel::wiki::builder::WikiSummary, Option<TopicTarget>),
    patches: Vec<WikiPatch>,
) -> Outcome {
    router.apply(job, &read.0, read.1.as_ref(), patches).await?;
    Ok(())
}

pub async fn a_sub_topic_rewrite_replaces_its_section(
    stores: RouteStores<'_>,
    first: &ExtractWiki,
    second: &ExtractWiki,
) -> Outcome {
    let infra = stores.pages.create(new_topic("Infra", &[], None)?).await?;
    let certificates = stores
        .pages
        .create(new_topic("Certificates", &[], Some(infra.id))?)
        .await?;
    stores.place(&certificates).await?;
    let old = "Certificates renew every week.";
    stores
        .route(first, vec![patch("infra", "Certificates", old)?])
        .await?;
    let read = target(stores, second).await?;
    let current = read
        .1
        .as_ref()
        .and_then(|topic| topic.current.as_ref())
        .and_then(|current| current.section.clone());
    same("the model sees the section", current.as_deref(), Some(old))?;
    let new = "Certificates renew every day since the step-ca move.";
    apply(
        &stores.router(),
        second,
        &read,
        vec![patch("infra", "Certificates", new)?],
    )
    .await?;
    let page = stores.pages.page(&infra).await?;
    same(
        "section rewritten, not appended",
        page.sections
            .iter()
            .map(|section| section.body.as_str())
            .collect::<Vec<_>>(),
        vec![new],
    )?;
    Ok(())
}

pub async fn concurrent_rewrites_of_one_page_keep_both(
    stores: RouteStores<'_>,
    first: &ExtractWiki,
    second: &ExtractWiki,
) -> Outcome {
    let acmekit = stores
        .pages
        .create(new_topic("AcmeKit", &[], None)?)
        .await?;
    stores.place(&acmekit).await?;
    let one = target(stores, first).await?;
    let two = target(stores, second).await?;
    let mut ships = patch("acmekit", "AcmeKit", "AcmeKit ships on Fridays.")?;
    ships.article = Some("### Releases\n\nReleases go out on Fridays.".to_owned());
    let mut hosts = patch("acmekit", "AcmeKit", "AcmeKit builds on the build box.")?;
    hosts.article = Some("### Builds\n\nBuilds run on the build box.".to_owned());
    let router = stores.router();
    let (left, right) = tokio::join!(
        apply(&router, first, &one, vec![ships]),
        apply(&router, second, &two, vec![hosts])
    );
    left?;
    right?;
    let page = stores.pages.page(&acmekit).await?;
    for kept in [
        "AcmeKit ships on Fridays.",
        "AcmeKit builds on the build box.",
    ] {
        same(kept, page.summary.contains(kept), true)?;
    }
    for kept in [
        "Releases go out on Fridays.",
        "Builds run on the build box.",
    ] {
        same(kept, page.article.contains(kept), true)?;
    }
    Ok(())
}

pub async fn an_entity_mention_is_merged_into_its_page(
    stores: RouteStores<'_>,
    first: &ExtractWiki,
    second: &ExtractWiki,
) -> Outcome {
    let built = "The build box runs the nightly AcmeKit builds.";
    let mention = host(built)?;
    stores.mentioned_elsewhere(first, &mention).await?;
    stores.route(first, vec![mention]).await?;
    let entity = stores
        .named(TopicKind::Entity, "AcmeKit build box")
        .await?
        .ok_or("the second mention creates the entity")?;
    let merger = RecordingMerger::default();
    let router = stores.router_with(&merger);
    let read = {
        let WikiWork::Pending(summary) = stores.wiki.wiki_work(second).await? else {
            return Err("the summary was already extracted".into());
        };
        (summary, None)
    };
    apply(
        &router,
        second,
        &read,
        vec![host("It now has 64 GB of memory.")?],
    )
    .await?;
    let seen = merger.seen.lock().clone();
    same("one merge", seen.len(), 1)?;
    let input = seen.first().ok_or("merge input")?;
    same("current summary sent", input.summary.as_str(), built)?;
    same(
        "mention sent",
        input
            .material
            .iter()
            .any(|text| text.contains("64 GB of memory")),
        true,
    )?;
    let page = stores.pages.page(&entity).await?;
    same(
        "merged summary replaces",
        page.summary.as_str(),
        format!("{built} Merged.").as_str(),
    )?;
    Ok(())
}
