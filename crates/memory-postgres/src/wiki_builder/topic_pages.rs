use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::topics::{PageSection, TopicId, TopicPage, TopicPageStore};
use kernel::wiki::builder::WikiStoreError;
use kernel::wiki::consolidate::{WikiConsolidateStore, WikiFold};
use wire::gateway::wiki::WikiSlug;
use wire::wiki::{ArticleSection, FoldMark, WikiPage, WikiSectionRef};

use super::{lock_slug, query_error, settle, topics};
use crate::PostgresMemory;
use crate::wiki_writer::{LockedRepository, WikiRepository};

#[derive(Debug, sqlx::FromRow)]
struct PageRow {
    title: String,
    current_state: String,
    article: String,
}

#[async_trait]
impl TopicPageStore for PostgresMemory {
    async fn topic_page(&self, slug: &WikiSlug) -> Result<Option<TopicPage>, WikiStoreError> {
        let repository = self.repository().await?;
        let read = repository
            .read(slug.as_ref())
            .await
            .map_err(WikiStoreError::from);
        match settle(read, repository.release().await)? {
            Some(page) => topic_page(page).map(Some),
            None => self.row_page(slug).await,
        }
    }

    async fn page_taken(&self, slug: &WikiSlug) -> Result<bool, WikiStoreError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM rh_wiki_topics WHERE slug = $1)
                 OR EXISTS (SELECT 1 FROM rh_wiki_redirects WHERE from_slug = $1)",
        )
        .bind(slug.as_ref())
        .fetch_one(&self.pool)
        .await
        .map_err(query_error)
    }

    async fn save_topic_page(
        &self,
        topic: TopicId,
        page: &TopicPage,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        let mut repository = self.repository().await?;
        let saved = self.save_locked(&mut repository, topic, page, now).await;
        settle(saved, repository.release().await)
    }

    async fn redirect_page(
        &self,
        from: &WikiSlug,
        to: &WikiSlug,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        sqlx::query(
            "INSERT INTO rh_wiki_redirects (from_slug, to_slug, created_at)
             SELECT $1, $2, $3
              WHERE NOT EXISTS (SELECT 1 FROM rh_wiki_topics WHERE slug = $1)
             ON CONFLICT (from_slug) DO UPDATE SET to_slug = EXCLUDED.to_slug
              WHERE rh_wiki_redirects.to_slug <> EXCLUDED.to_slug",
        )
        .bind(from.as_ref())
        .bind(to.as_ref())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(query_error)?;
        Ok(())
    }

    async fn fold_page(
        &self,
        child: &WikiSlug,
        into: &WikiSlug,
        mark: FoldMark,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        let fold = WikiFold {
            cluster: into.as_ref().to_owned(),
            parent: into.as_ref().to_owned(),
            children: vec![child.as_ref().to_owned()],
            mark,
        };
        self.fold_wiki(&fold, now).await.map(drop)
    }
}

impl PostgresMemory {
    async fn repository(&self) -> Result<LockedRepository, WikiStoreError> {
        let directory = self
            .wiki_directory
            .as_ref()
            .ok_or(WikiStoreError::DirectoryMissing)?;
        Ok(WikiRepository::new(directory.clone()).lock().await?)
    }

    async fn row_page(&self, slug: &WikiSlug) -> Result<Option<TopicPage>, WikiStoreError> {
        let row = sqlx::query_as::<_, PageRow>(
            "SELECT title, current_state, article FROM rh_wiki_topics WHERE slug = $1",
        )
        .bind(slug.as_ref())
        .fetch_optional(&self.pool)
        .await
        .map_err(query_error)?;
        Ok(row.map(|row| TopicPage {
            slug: slug.clone(),
            title: row.title,
            summary: row.current_state,
            article: row.article,
            sections: Vec::new(),
        }))
    }

    async fn save_locked(
        &self,
        repository: &mut LockedRepository,
        topic: TopicId,
        page: &TopicPage,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        lock_slug(&mut tx, page.slug.as_ref()).await?;
        let mut stored = match repository.read(page.slug.as_ref()).await? {
            Some(stored) => stored,
            None => WikiPage::empty(page.slug.clone(), page.title.clone()),
        };
        page.title.clone_into(&mut stored.meta.title);
        page.summary.clone_into(&mut stored.current_state);
        page.article.clone_into(&mut stored.article);
        stored.meta.sections = page
            .sections
            .iter()
            .filter_map(|section| {
                section.topic.map(|topic| WikiSectionRef {
                    topic: topic.to_string(),
                    heading: section.heading.clone(),
                })
            })
            .collect();
        stored.extra_sections = page
            .sections
            .iter()
            .map(|section| ArticleSection {
                heading: section.heading.clone(),
                body: section.body.clone(),
            })
            .collect();
        let message = format!(
            "wiki({}): sync topic page\n\nPipeline: wiki-v7",
            page.slug.as_ref()
        );
        let git_sha = repository.fold(&stored, &[], &message).await?;
        topics::store_topic(&mut tx, &stored, &git_sha, now).await?;
        sqlx::query("UPDATE rh_wiki_topics SET topic_id = NULL WHERE topic_id = $1 AND slug <> $2")
            .bind(topic.uuid())
            .bind(page.slug.as_ref())
            .execute(&mut *tx)
            .await
            .map_err(query_error)?;
        sqlx::query("UPDATE rh_wiki_topics SET topic_id = $1 WHERE slug = $2")
            .bind(topic.uuid())
            .bind(page.slug.as_ref())
            .execute(&mut *tx)
            .await
            .map_err(query_error)?;
        tx.commit().await.map_err(query_error)
    }
}

fn topic_page(page: WikiPage) -> Result<TopicPage, WikiStoreError> {
    let refs = page.meta.sections;
    let sections = page
        .extra_sections
        .into_iter()
        .map(|section| {
            let topic = refs
                .iter()
                .find(|known| known.heading == section.heading)
                .map(|known| known.topic.parse::<TopicId>())
                .transpose()
                .map_err(|source| WikiStoreError::Query(Box::new(source)))?;
            Ok(PageSection {
                topic,
                heading: section.heading,
                body: section.body,
            })
        })
        .collect::<Result<_, WikiStoreError>>()?;
    Ok(TopicPage {
        slug: page.meta.slug,
        title: page.meta.title,
        summary: page.current_state,
        article: page.article,
        sections,
    })
}
