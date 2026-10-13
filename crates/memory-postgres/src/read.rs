use crate::error::Error;
use crate::search::SearchOptions;
use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use kernel::memory::BrowseFilter;
use kernel::memory::SearchMode;
use std::str::FromStr as _;
use uuid::Uuid;
use wire::JsonObject;
use wire::gateway::memory::BrowseWindow;
use wire::gateway::memory::MemoryBrowseOrder;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;
use wire::gateway::memory::MemoryStatsResponse;
use wire::gateway::memory::RecentSession;
use wire::gateway::memory::TopTool;
use wire::memory::MemoryEntry;
use wire::message::Role;

const CONTEXT_MAX_TOKENS: usize = 4_000;
const CONTEXT_RECENT_ROWS: i64 = 5;
const CONTEXT_RELEVANT_LIMIT: u32 = 10;
const CONTEXT_LINE_CAP: usize = 500;
const CONTEXT_DEDUP_PREFIX: usize = 300;
const WIKI_LEAD_CAP: usize = 1_200;
const WIKI_ARTICLE_CAP: usize = 800;
const WIKI_TOPICS_LIMIT: i64 = 3;

#[derive(sqlx::FromRow)]
pub(crate) struct MessageEntryRow {
    pub(crate) id: Option<String>,
    pub(crate) session_key: String,
    pub(crate) agent: String,
    pub(crate) channel: String,
    pub(crate) role_text: Option<String>,
    pub(crate) content: String,
    pub(crate) tool_name: Option<String>,
    pub(crate) tool_args: Option<String>,
    pub(crate) tool_result: Option<String>,
    pub(crate) metadata: Option<String>,
    pub(crate) created_at: Option<DateTime<Utc>>,
}

impl TryFrom<MessageEntryRow> for MemoryEntry {
    type Error = Error;

    fn try_from(row: MessageEntryRow) -> Result<Self, Self::Error> {
        let role_text = row
            .role_text
            .as_deref()
            .ok_or_else(|| Error::protocol("message row is missing its role"))?;
        let role = Role::from_str(role_text).map_err(|source| {
            Error::protocol(format!(
                "message row has an unknown role \"{role_text}\": {source}"
            ))
        })?;
        Ok(Self {
            id: row.id,
            session_id: row.session_key,
            agent: row.agent,
            channel: row.channel,
            role,
            content: row.content,
            tool_name: row.tool_name,
            tool_args: stored_json(row.tool_args, "tool_args")?,
            tool_result: row.tool_result,
            metadata: stored_json(row.metadata, "metadata")?,
            created_at: row.created_at,
        })
    }
}

fn stored_json(raw: Option<String>, column: &'static str) -> Result<Option<JsonObject>, Error> {
    let Some(text) = raw else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| Error::protocol(format!("stored {column} is not JSON: {source}")))
}

fn utf16_prefix(text: &str, max: usize) -> String {
    let mut units = 0usize;
    for (idx, ch) in text.char_indices() {
        if units + ch.len_utf16() > max {
            return text[..idx].to_string();
        }
        units += ch.len_utf16();
    }
    text.to_string()
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn count(value: i64) -> Result<u64, Error> {
    u64::try_from(value).map_err(|source| Error::protocol(source.to_string()))
}

pub(crate) fn window_bounds(
    window: BrowseWindow,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    kernel::memory_core::resolve_window(window, Utc::now(), &Local)
}

fn filter_bounds(filter: &BrowseFilter) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    match (filter.since, filter.before, filter.window) {
        (None, None, Some(window)) => window_bounds(window),
        _ => (filter.since, filter.before),
    }
}

fn push_tag_condition(
    query: &mut sqlx::QueryBuilder<sqlx::Postgres>,
    tag: &kernel::memory::TagLiteral,
) {
    query.push(
        " and m.conversation_id in (select entity_id from rh_tags \
         where key = ",
    );
    query.push_bind(tag.key.clone());
    query.push(" and value = ");
    query.push_bind(tag.value.clone());
    query.push(" and state = 'accepted' and entity_type = 'conversation')");
}

impl crate::PostgresMemory {
    pub(crate) async fn browse_port(
        &self,
        filter: &BrowseFilter,
    ) -> Result<Vec<MemoryEntry>, Error> {
        let mut query = sqlx::QueryBuilder::new(
            "select m.id::text as id, c.session_key, m.agent, m.channel, m.role as role_text, \
             m.content, m.tool_name, m.tool_args::text as tool_args, m.tool_result, \
             m.metadata::text as metadata, m.created_at \
             from rh_messages m join rh_conversations c on c.id = m.conversation_id where true",
        );
        if !filter.include_tools {
            query.push(" and m.role <> 'tool'");
        }
        if let Some(conversation_id) = filter.conversation_id {
            query.push(" and m.conversation_id = ");
            query.push_bind(conversation_id);
        }
        if let Some(agent) = &filter.agent {
            query.push(" and m.agent = ");
            query.push_bind(agent.clone());
        }
        if let Some(role) = filter.role {
            query.push(" and m.role = ");
            query.push_bind(role.to_string());
        }
        if let Some(tool_name) = &filter.tool_name {
            query.push(" and m.tool_name = ");
            query.push_bind(tool_name.clone());
        }
        if let Some(tag) = &filter.tag {
            push_tag_condition(&mut query, tag);
        }
        let (since, before) = filter_bounds(filter);
        if let Some(since) = since {
            query.push(" and m.created_at >= ");
            query.push_bind(since);
        }
        if let Some(before) = before {
            query.push(" and m.created_at < ");
            query.push_bind(before);
        }
        let direction = match filter.order {
            MemoryBrowseOrder::Asc => "asc",
            MemoryBrowseOrder::Desc => "desc",
        };
        query.push(format!(
            " order by m.created_at {direction} limit {}",
            i64::from(filter.limit.get())
        ));
        let rows = query
            .build_query_as::<MessageEntryRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("browse", source))?;
        let entries = rows
            .into_iter()
            .map(MemoryEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub async fn memory_get_full(&self, id: &str) -> Result<Option<MemoryEntry>, Error> {
        let Ok(uuid) = Uuid::parse_str(id) else {
            return Ok(None);
        };
        let record = sqlx::query_as!(
            MessageEntryRow,
            "select m.id::text as id, c.session_key as \"session_key!\", \
             m.agent as \"agent!\", m.channel as \"channel!\", m.role as role_text, \
             m.content as \"content!\", m.tool_name, m.tool_args::text as tool_args, \
             m.tool_result, m.metadata::text as metadata, m.created_at \
             from rh_messages m join rh_conversations c on c.id = m.conversation_id \
             where m.id = $1",
            uuid
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("get full", source))?
        .map(MemoryEntry::try_from)
        .transpose()?;
        Ok(record)
    }

    pub async fn memory_stats(&self) -> Result<MemoryStatsResponse, Error> {
        let totals = sqlx::query!(
            "select
                (select count(*) from rh_conversations) as \"conversations!\",
                (select count(*) from rh_messages) as \"messages!\",
                (select count(*) from rh_messages
                  where role = 'tool' or tool_name is not null) as \"tool_calls!\",
                (select count(*) from rh_summaries) as \"summaries!\",
                (select count(embedding) from rh_messages) as \"embedded_messages!\",
                (select count(*) from rh_messages
                  where embedding is null
                    and embed_status is distinct from 'unembeddable'
                    and embed_status is distinct from 'failed'
                    and (length(coalesce(content, '')) > 0
                         or length(coalesce(tool_result, '')) > 0))
                  + (select count(*) from rh_summaries
                      where embedding is null
                        and embed_status is distinct from 'unembeddable'
                        and embed_status is distinct from 'failed')
                  as \"embed_queue_depth!\",
                (select count(*) from rh_messages where embed_status = 'failed')
                  + (select count(*) from rh_summaries where embed_status = 'failed')
                  as \"failed_embeddings!\""
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|source| Error::operation("stats", source))?;
        let tool_rows = sqlx::query!(
            "select tool_name as \"tool_name!\", count(*) as \"n!\" from rh_messages
              where tool_name is not null
              group by tool_name order by count(*) desc limit 12"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("stats", source))?;
        let top_tools: Vec<TopTool> = tool_rows
            .into_iter()
            .map(|row| -> Result<TopTool, Error> {
                Ok(TopTool {
                    tool: row.tool_name,
                    count: count(row.n)?,
                })
            })
            .collect::<Result<_, _>>()?;
        let session_rows = sqlx::query!(
            "select c.id::text as \"session_id!\", c.title, c.agent, c.updated_at,
                    (select count(*) from rh_messages m where m.conversation_id = c.id)
                      as \"messages!\"
               from rh_conversations c order by c.updated_at desc limit 12"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("stats", source))?;
        let recent_sessions: Vec<RecentSession> = session_rows
            .into_iter()
            .map(|row| -> Result<RecentSession, Error> {
                Ok(RecentSession {
                    session_id: row.session_id,
                    title: row.title,
                    agent: row.agent,
                    last_active: row.updated_at,
                    messages: count(row.messages)?,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(MemoryStatsResponse {
            conversations: count(totals.conversations)?,
            messages: count(totals.messages)?,
            tool_calls: count(totals.tool_calls)?,
            summaries: count(totals.summaries)?,
            embed_queue_depth: count(totals.embed_queue_depth)?,
            embedded_messages: count(totals.embedded_messages)?,
            failed_embeddings: count(totals.failed_embeddings)?,
            top_tools,
            recent_sessions,
        })
    }

    #[expect(clippy::too_many_lines, reason = "three sections built in order")]
    pub async fn context_for_turn(
        &self,
        session: &str,
        now: DateTime<Utc>,
    ) -> Result<String, Error> {
        let conversation = sqlx::query!(
            "select id::text as id, agent from rh_conversations where session_key = $1 limit 1",
            session
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("context", source))?;
        let Some(conversation) = conversation else {
            return Ok(String::new());
        };
        let agent = conversation.agent;
        let mut dedup = std::collections::HashSet::new();
        let mut budget = CONTEXT_MAX_TOKENS;
        let mut sections: Vec<String> = Vec::new();

        let recent_rows = sqlx::query!(
            "select m.role, m.content from rh_messages m
               join rh_conversations c on c.id = m.conversation_id
              where c.active
                and c.session_key not like 'heartbeat:%'
                and c.agent = $1
              order by m.created_at desc limit $2",
            agent,
            CONTEXT_RECENT_ROWS
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("context", source))?;
        let mut recent_lines: Vec<String> = Vec::new();
        for row in recent_rows.iter().rev() {
            let line = format!(
                "[{}] {}",
                row.role,
                utf16_prefix(&row.content, CONTEXT_LINE_CAP)
            );
            let key = utf16_prefix(&row.content, CONTEXT_DEDUP_PREFIX);
            if !dedup.insert(key) {
                continue;
            }
            if !push_within_budget(&mut budget, &line) {
                break;
            }
            recent_lines.push(line);
        }
        if !recent_lines.is_empty() {
            let mut section = String::from("## Recent");
            for line in recent_lines {
                section.push('\n');
                section.push_str(&line);
            }
            sections.push(section);
        }

        let query_text = sqlx::query_scalar!(
            "select content from rh_messages
              where conversation_id = (select id from rh_conversations where session_key = $1 limit 1)
                and role = 'user'
              order by created_at desc limit 1",
            session
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("context", source))?
        .unwrap_or_default();

        if self.wiki_available() {
            match self
                .wiki_section(&query_text, &mut dedup, &mut budget)
                .await
            {
                Ok(section) => {
                    if let Some(section) = section {
                        sections.push(section);
                    }
                }
                Err(error) => {
                    if error.to_string().contains("does not exist") {
                        self.set_wiki_available(false);
                    }
                }
            }
        }

        let hits = self
            .search_full(&SearchOptions {
                text: query_text.clone(),
                mode: SearchMode::Hybrid,
                scope: MemoryScope::Both,
                limit: CONTEXT_RELEVANT_LIMIT,
                agent: Some(agent.clone()),
                tag: None,
            })
            .await?;
        let mut relevant_lines: Vec<String> = Vec::new();
        for hit in hits {
            let key = utf16_prefix(&hit.content, CONTEXT_DEDUP_PREFIX);
            if !dedup.insert(key) {
                continue;
            }
            let when = relative_when(now - hit.created_at);
            let line = match hit.source {
                MemoryHitSource::Message => {
                    let role = hit
                        .role
                        .ok_or_else(|| Error::protocol("message hit is missing its role"))?;
                    let agent_name = hit.agent.clone().unwrap_or_else(|| agent.clone());
                    format!(
                        "[{agent_name}/{role}, {when} · {}] {}",
                        hit.created_at.format("%Y-%m-%d %H:%M"),
                        utf16_prefix(&hit.content, CONTEXT_LINE_CAP)
                    )
                }
                MemoryHitSource::Summary => {
                    let label = hit
                        .kind
                        .as_deref()
                        .map_or_else(|| "summary".to_owned(), |kind| format!("summary/{kind}"));
                    format!(
                        "[{label}, {when} · {}] {}",
                        hit.created_at.format("%Y-%m-%d %H:%M"),
                        utf16_prefix(&hit.content, CONTEXT_LINE_CAP)
                    )
                }
            };
            if !push_within_budget(&mut budget, &line) {
                break;
            }
            relevant_lines.push(line);
        }
        if !relevant_lines.is_empty() {
            let mut section = String::from("## Relevant Context");
            for line in relevant_lines {
                section.push('\n');
                section.push_str(&line);
            }
            sections.push(section);
        }
        Ok(sections.join("\n"))
    }

    async fn wiki_section(
        &self,
        query_text: &str,
        dedup: &mut std::collections::HashSet<String>,
        budget: &mut usize,
    ) -> Result<Option<String>, Error> {
        if query_text.trim().is_empty() {
            return Ok(None);
        }
        let rows = sqlx::query!(
            "select slug, title, current_state, article from rh_wiki_topics
              where content_tsv @@ plainto_tsquery('english', $1)
              order by ts_rank_cd(content_tsv, plainto_tsquery('english', $1)) desc
              limit $2",
            query_text,
            WIKI_TOPICS_LIMIT
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki search", source))?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut lines: Vec<String> = Vec::new();
        for row in rows {
            let key = utf16_prefix(&row.current_state, CONTEXT_DEDUP_PREFIX);
            dedup.insert(key);
            let mut block = format!("**{}** (wiki:{})", row.title, row.slug);
            let lead = utf16_prefix(&row.current_state, WIKI_LEAD_CAP);
            if !lead.trim().is_empty() {
                block.push('\n');
                block.push_str(&lead);
            }
            let article = utf16_prefix(&row.article, WIKI_ARTICLE_CAP);
            if !article.trim().is_empty() {
                block.push('\n');
                block.push_str(&article);
            }
            lines.push(block);
        }
        let mut section = String::from("## Wiki (curated state)");
        for line in lines {
            if !push_within_budget(budget, &line) {
                break;
            }
            section.push('\n');
            section.push_str(&line);
        }
        Ok(Some(section))
    }
}

fn push_within_budget(budget: &mut usize, line: &str) -> bool {
    let tokens = utf16_len(line).div_ceil(4);
    if tokens > *budget {
        return false;
    }
    *budget -= tokens;
    true
}

fn relative_when(delta: chrono::Duration) -> String {
    let minutes = delta.num_minutes().max(0);
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes} minutes ago")
    } else if minutes < 60 * 24 {
        format!("{} hours ago", minutes / 60)
    } else if minutes < 60 * 24 * 30 {
        format!("{} days ago", minutes / (60 * 24))
    } else {
        format!("{} months ago", minutes / (60 * 24 * 30))
    }
}
