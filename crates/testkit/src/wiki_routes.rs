use std::str::FromStr;

use async_trait::async_trait;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Topic;
use kernel::topics::TopicKey;
use kernel::transcript::ListLimit;
use kernel::transcript::SessionLister;
use kernel::transcript::SessionRenameError;
use kernel::transcript::StoreSession;
use kernel::transcript::ToolOutputRead;
use kernel::transcript::TranscriptRead;
use kernel::transcript::TranscriptReaderError;
use kernel::wiki::WikiLookup;
use kernel::wiki::builder::EntityKind;
use kernel::wiki::builder::ExtractWiki;
use kernel::wiki::builder::WikiAction;
use kernel::wiki::builder::WikiBuilderStore;
use kernel::wiki::builder::WikiPatch;
use kernel::wiki::builder::WikiWork;
use kernel::wiki::route::TopicPorts;
use kernel::wiki::route::WikiRouter;
use wire::gateway::wiki::WikiSlug;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

use crate::topic_pages::PageStores;
use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::new_topic;
use crate::topics::same;

pub const SESSION_KEY: &str = "claude-code:00000000-0000-4000-8000-000000000042";

type Found<T> = Result<T, Box<dyn std::error::Error>>;

#[expect(
    missing_debug_implementations,
    reason = "the stores are kernel ports without Debug"
)]
#[derive(Clone, Copy)]
pub struct RouteStores<'a> {
    pub pages: PageStores<'a>,
    pub wiki: &'a dyn WikiBuilderStore,
}

struct NoParents;

#[async_trait]
impl SessionLister for NoParents {
    async fn list(&self, _harnesses: &[HarnessId], _limit: ListLimit) -> Vec<StoreSession> {
        Vec::new()
    }

    async fn describe(
        &self,
        _harness: HarnessId,
        _native: &str,
        _parent: Option<&str>,
    ) -> Result<Option<StoreSession>, TranscriptReaderError> {
        Ok(None)
    }

    async fn store_cwd(
        &self,
        _harness: HarnessId,
        _native: &str,
    ) -> Result<Option<String>, TranscriptReaderError> {
        Ok(None)
    }

    async fn transcript(
        &self,
        _harness: HarnessId,
        _native: &str,
    ) -> Result<TranscriptRead, TranscriptReaderError> {
        Ok(TranscriptRead::NoSource)
    }

    async fn rename(
        &self,
        _harness: HarnessId,
        _native: &str,
        _title: &str,
    ) -> Result<StoreSession, SessionRenameError> {
        Err(SessionRenameError::Missing)
    }

    async fn tool_output(
        &self,
        _harness: HarnessId,
        _native: &str,
        _tool_call_id: &str,
    ) -> Result<ToolOutputRead, TranscriptReaderError> {
        Ok(ToolOutputRead::NoSource)
    }
}

impl RouteStores<'_> {
    async fn place(&self, topic: &Topic) -> Outcome {
        self.pages
            .topics
            .place(Placement {
                session: SessionId::from_str(SESSION_KEY)?,
                topic: topic.id,
                by: PlacementSource::You,
                at: at(1),
            })
            .await?;
        Ok(())
    }

    async fn route(&self, job: &ExtractWiki, patches: Vec<WikiPatch>) -> Outcome {
        let router = WikiRouter {
            wiki: self.wiki,
            topics: Some(TopicPorts {
                topics: self.pages.topics,
                pages: self.pages.pages,
                lineage: &NoParents,
            }),
            now: at(20),
        };
        let WikiWork::Pending(summary) = self.wiki.wiki_work(job).await? else {
            return Err("the summary was already extracted".into());
        };
        let target = router.target(&summary).await?;
        router.apply(job, target.as_ref(), patches).await?;
        self.pages.settle().await
    }

    async fn topic(&self, name: &str) -> Found<Topic> {
        Ok(self
            .pages
            .topics
            .find(&TopicKey::parse(name)?)
            .await?
            .ok_or_else(|| format!("no topic {name}"))?)
    }
}

fn patch(slug: &str, title: &str, summary: &str) -> Found<WikiPatch> {
    Ok(WikiPatch {
        action: WikiAction::Create,
        slug: WikiSlug::try_from(slug.to_owned())?,
        title: title.to_owned(),
        aliases: Vec::new(),
        tags: Vec::new(),
        entities: Vec::new(),
        summary: summary.to_owned(),
        article: None,
        related: None,
        history_entry: None,
        kind: None,
    })
}

pub async fn a_placed_session_writes_its_topic_page_and_no_other_topics_page(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
) -> Outcome {
    let acmekit = stores
        .pages
        .create(new_topic("AcmeKit", &[], None)?)
        .await?;
    stores
        .pages
        .create(new_topic("Billing", &[], None)?)
        .await?;
    stores.place(&acmekit).await?;
    let state = "AcmeKit ships every Friday.";
    stores
        .route(
            job,
            vec![
                patch("acmekit", "AcmeKit deploys", state)?,
                patch("rivet-billing", "Billing", "Invoices go out monthly.")?,
            ],
        )
        .await?;
    let home = stores.pages.shown("acmekit").await?;
    same("topic page state", home.current_state.as_str(), state)?;
    same("topic page title", home.title.as_str(), "AcmeKit")?;
    same(
        "other topic page",
        stores.pages.shown("billing").await?.current_state,
        String::new(),
    )?;
    same(
        "unbound page",
        stores.pages.shown("rivet-billing").await?.topic_id,
        None,
    )?;
    Ok(())
}

pub async fn the_topic_page_is_cited_without_a_proposal(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
) -> Outcome {
    let topic = stores
        .pages
        .create(new_topic("AcmeKit", &[], None)?)
        .await?;
    stores.place(&topic).await?;
    stores.route(job, Vec::new()).await?;
    let cited: Vec<String> = stores
        .pages
        .shown("acmekit")
        .await?
        .citations
        .into_iter()
        .map(|citation| citation.summary_id)
        .collect();
    same("citations", cited, vec![job.summary_id.to_string()])?;
    Ok(())
}

pub async fn a_host_is_filed_as_a_section_under_hosts(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
) -> Outcome {
    let text = "The build box runs the nightly AcmeKit builds.";
    let mut host = patch("acmekit-build-box", "AcmeKit build box", text)?;
    host.kind = Some(EntityKind::Host);
    stores.route(job, vec![host]).await?;
    let hosts = stores.topic("Hosts").await?;
    same("category root", hosts.parent, None)?;
    let entity = stores.topic("AcmeKit build box").await?;
    same("entity sub-topic", entity.parent, Some(hosts.id))?;
    let sections = stores.pages.page(&hosts).await?.sections;
    same(
        "section",
        sections
            .iter()
            .map(|section| {
                (
                    section.topic,
                    section.heading.as_str(),
                    section.body.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(Some(entity.id), "AcmeKit build box", text)],
    )?;
    same(
        "entity page",
        stores.pages.lookup("acmekit-build-box").await?,
        WikiLookup::Missing,
    )?;
    Ok(())
}

pub async fn a_sub_topic_summary_lands_in_its_section(
    stores: RouteStores<'_>,
    job: &ExtractWiki,
) -> Outcome {
    let infra = stores.pages.create(new_topic("Infra", &[], None)?).await?;
    let certificates = stores
        .pages
        .create(new_topic("Certificates", &[], Some(infra.id))?)
        .await?;
    stores.place(&certificates).await?;
    let text = "Certificates renew every week.";
    stores
        .route(job, vec![patch("infra", "Certificates", text)?])
        .await?;
    let page = stores.pages.page(&infra).await?;
    same("root summary", page.summary.as_str(), "")?;
    same(
        "section",
        page.sections
            .iter()
            .map(|section| (section.topic, section.body.as_str()))
            .collect::<Vec<_>>(),
        vec![(Some(certificates.id), text)],
    )?;
    Ok(())
}
