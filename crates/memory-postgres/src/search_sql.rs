use crate::error::Error;
use crate::search::ArmKind;
use crate::search::ArmRow;
use crate::search::SearchOptions;
use crate::search::TagFilter;
use chrono::Utc;
use kernel::memory_core::HYBRID_MIN_CONTENT_LEN;
use kernel::memory_core::hybrid_pool;
use kernel::memory_core::recency_boost;
use kernel::memory_core::should_trigram_fallback;
use pgvector::HalfVector;
use sqlx::QueryBuilder;
use std::collections::BTreeMap;
use std::str::FromStr as _;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;
use wire::message::Role;

const CHUNK_POOL_MULTIPLIER: i64 = 4;
const CHUNK_POOL_MAX: i64 = 400;

fn chunk_pool(limit: u32) -> i64 {
    (hybrid_pool(limit) * CHUNK_POOL_MULTIPLIER).min(CHUNK_POOL_MAX)
}

fn semantic_floor(alias: &str) -> String {
    format!("least(length(btrim({alias}.content)) / 1000.0, 1.0)")
}

fn recency_columns(alias: &str, is_message: bool) -> String {
    let importance = if is_message {
        format!(
            "(CASE WHEN {alias}.role = 'system' THEN 0.5 \
                    WHEN {alias}.tool_name IS NOT NULL THEN 0.3 \
                    WHEN {alias}.role = 'user' THEN 0.7 \
                    ELSE 0.6 END)"
        )
    } else {
        "0.6".to_string()
    };
    format!(
        "(EXTRACT(EPOCH FROM (now() - {alias}.created_at)) / 86400.0)::float8 AS age_days, \
         COALESCE({alias}.access_count, 0) AS access_count, \
         ({importance})::float8 AS importance"
    )
}

fn summary_kind(raw: Option<String>) -> Result<Option<kernel::memory::SummaryKind>, Error> {
    raw.map(|kind| {
        kind.parse()
            .map_err(|source| Error::stored("summary kind", source))
    })
    .transpose()
}

fn role_of(raw: Option<String>) -> Result<Option<Role>, Error> {
    raw.map(|role| Role::from_str(&role).map_err(|source| Error::stored("message role", source)))
        .transpose()
}

impl TagFilter {
    pub(crate) fn push_condition(
        &self,
        query: &mut QueryBuilder<sqlx::Postgres>,
        is_message: bool,
    ) {
        let (outer, inner) = if is_message {
            ("m.conversation_id", "conversation")
        } else {
            ("s.id", "summary")
        };
        query.push(format!(
            "{outer} IN (SELECT entity_id FROM rh_tags WHERE key = "
        ));
        query.push_bind(self.key.clone());
        query.push(" AND value = ");
        query.push_bind(self.value.clone());
        query.push(format!(
            " AND state = 'accepted' AND entity_type = '{inner}')"
        ));
    }
}

fn push_time_conditions(
    query: &mut QueryBuilder<sqlx::Postgres>,
    options: &SearchOptions,
    alias: &str,
) {
    if let Some(since) = options.since {
        query.push(format!(" AND {alias}.created_at >= "));
        query.push_bind(since);
    }
    if let Some(before) = options.before {
        query.push(format!(" AND {alias}.created_at < "));
        query.push_bind(before);
    }
}

fn push_agent_condition(query: &mut QueryBuilder<sqlx::Postgres>, agent: &str, is_message: bool) {
    if is_message {
        query.push(" AND m.agent = ");
        query.push_bind(agent.to_owned());
    } else {
        query.push(
            " AND EXISTS (SELECT 1 FROM rh_conversations c \
             WHERE c.id = s.conversation_id AND c.agent = ",
        );
        query.push_bind(agent.to_owned());
        query.push(")");
    }
}

fn literal_tokens(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        if should_trigram_fallback(token) && !tokens.iter().any(|seen| seen == token) {
            tokens.push(token.to_owned());
        }
    }
    let whole = text.trim();
    if tokens.is_empty() && !whole.is_empty() {
        tokens.push(whole.to_owned());
    }
    tokens
}

fn like_escaped(token: &str) -> String {
    let mut escaped = String::with_capacity(token.len());
    for character in token.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn push_contains(query: &mut QueryBuilder<sqlx::Postgres>, column: &str, token: &str) {
    query.push(format!("{column} ILIKE '%' || "));
    query.push_bind(like_escaped(token));
    query.push(" || '%' ESCAPE '\\'");
}

fn push_any_contains(query: &mut QueryBuilder<sqlx::Postgres>, column: &str, tokens: &[String]) {
    query.push("(");
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        push_contains(query, column, token);
    }
    query.push(")");
}

fn push_literal_score(
    query: &mut QueryBuilder<sqlx::Postgres>,
    tokens: &[String],
    is_message: bool,
) {
    query.push("((0");
    for token in tokens {
        query.push(" + (CASE WHEN ");
        if is_message {
            push_contains(query, "m.content", token);
            query.push(" OR ");
            push_contains(query, "coalesce(m.tool_result, '')", token);
        } else {
            push_contains(query, "s.content", token);
        }
        query.push(" THEN 1 ELSE 0 END)");
    }
    query.push(format!(")::float8 / {})", tokens.len()));
}

fn push_literal_match(
    query: &mut QueryBuilder<sqlx::Postgres>,
    tokens: &[String],
    is_message: bool,
) {
    if is_message {
        query.push(" AND m.id IN (SELECT hit.id FROM rh_messages hit WHERE ");
        push_any_contains(query, "hit.content", tokens);
        query.push(
            " UNION SELECT hit.id FROM rh_messages hit \
             WHERE hit.tool_result IS NOT NULL AND length(hit.tool_result) > 0 AND ",
        );
        push_any_contains(query, "hit.tool_result", tokens);
        query.push(")");
    } else {
        query.push(" AND ");
        push_any_contains(query, "s.content", tokens);
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArmSqlRow {
    chunk: Option<sqlx::types::Json<wire::memory::MemoryChunkMatch>>,
    id: String,
    content: String,
    role_or_kind: Option<String>,
    agent: Option<String>,
    tool_name: Option<String>,
    conversation_id: Option<String>,
    session_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
    score: f64,
    semantic: f64,
    age_days: f64,
    #[sqlx(try_from = "i32")]
    access_count: u32,
    importance: f64,
}

impl ArmSqlRow {
    fn into_arm(self, is_message: bool) -> Result<ArmRow, Error> {
        let (role, kind) = if is_message {
            (role_of(self.role_or_kind)?, None)
        } else {
            (None, summary_kind(self.role_or_kind)?)
        };
        Ok(ArmRow {
            chunk: self.chunk.map(|chunk| chunk.0),
            id: self.id,
            source: if is_message {
                MemoryHitSource::Message
            } else {
                MemoryHitSource::Summary
            },
            content: self.content,
            role,
            agent: self.agent,
            kind,
            tool_name: self.tool_name,
            conversation_id: self.conversation_id,
            session_id: self.session_id,
            created_at: self.created_at,
            arm_score: self.score,
            semantic: self.semantic,
            boost: recency_boost(self.age_days, self.access_count, self.importance),
        })
    }
}

fn push_select_prefix(query: &mut QueryBuilder<sqlx::Postgres>, is_message: bool, chunk: bool) {
    query.push("SELECT ");
    query.push(if chunk {
        "jsonb_build_object('index', c.idx, 'total', (SELECT count(*) FROM rh_message_chunks all_chunks WHERE all_chunks.message_id = c.message_id), 'charStart', c.char_start, 'charEnd', c.char_end, 'content', c.content) AS chunk, "
    } else {
        "NULL::jsonb AS chunk, "
    });
    if is_message {
        query.push(
            "m.id::text, m.content, m.role AS role_or_kind, m.agent, m.tool_name, \
             m.conversation_id::text AS conversation_id, \
             (SELECT conv.session_key FROM rh_conversations conv WHERE conv.id = m.conversation_id) AS session_id, \
             m.created_at, ",
        );
    } else {
        query.push(
            "s.id::text, s.content, s.kind AS role_or_kind, NULL AS agent, \
             NULL AS tool_name, NULL AS conversation_id, NULL AS session_id, s.created_at, ",
        );
    }
}

fn push_embedded_semantic(
    query: &mut QueryBuilder<sqlx::Postgres>,
    distance_expr: &str,
    alias: &str,
    vector: &HalfVector,
) {
    query.push("coalesce(1.0 - (");
    query.push(distance_expr);
    query.push(" <=> ");
    query.push_bind(vector.clone());
    query.push("), ");
    query.push(semantic_floor(alias));
    query.push(")::float8 AS semantic, ");
}

struct ArmContext<'a> {
    options: &'a SearchOptions,
    tag_filter: Option<&'a TagFilter>,
}

const ESCAPE_HATCH_TIMEOUT: &str = "30s";

impl crate::PostgresMemory {
    pub(crate) async fn arm_rows(
        &self,
        options: &SearchOptions,
        kind: ArmKind,
        tag_filter: Option<&TagFilter>,
        query_vector: Option<&[f32]>,
    ) -> Result<Vec<ArmRow>, Error> {
        let context = ArmContext {
            options,
            tag_filter,
        };
        let mut rows = Vec::new();
        if matches!(options.scope, MemoryScope::Messages | MemoryScope::Both) {
            rows.extend(self.arm_table(&context, kind, true, query_vector).await?);
        }
        if matches!(options.scope, MemoryScope::Summaries | MemoryScope::Both) {
            rows.extend(self.arm_table(&context, kind, false, query_vector).await?);
        }
        rows.sort_by(|a, b| {
            let by_score = b.arm_score.total_cmp(&a.arm_score);
            if kind == ArmKind::Trigram {
                by_score.then_with(|| b.created_at.cmp(&a.created_at))
            } else {
                by_score
            }
        });
        Ok(rows)
    }

    #[expect(clippy::too_many_lines, reason = "one SQL builder covers both tables")]
    async fn arm_table(
        &self,
        context: &ArmContext<'_>,
        kind: ArmKind,
        is_message: bool,
        query_vector: Option<&[f32]>,
    ) -> Result<Vec<ArmRow>, Error> {
        let options = context.options;
        let tag_filter = context.tag_filter;
        let pool = hybrid_pool(options.limit);
        let alias = if is_message { "m" } else { "s" };
        let tokens = if kind == ArmKind::Trigram {
            literal_tokens(&options.text)
        } else {
            Vec::new()
        };
        if kind == ArmKind::Trigram && tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut query = QueryBuilder::new("");
        push_select_prefix(&mut query, is_message, false);
        match kind {
            ArmKind::Fts | ArmKind::FtsExplicit => {
                query.push(format!(
                    "ts_rank_cd({alias}.content_tsv, plainto_tsquery('english', "
                ));
                query.push_bind(options.text.clone());
                query.push(if is_message {
                    "), 32)::float8"
                } else {
                    "))::float8"
                });
            }
            ArmKind::Trigram => push_literal_score(&mut query, &tokens, is_message),
            ArmKind::Regex => {
                query.push("1.0::float8");
            }
        }
        query.push(" AS score, ");
        if let Some(vector) = query_vector.map(HalfVector::from_f32_slice) {
            let distance = format!("{alias}.embedding");
            push_embedded_semantic(&mut query, &distance, alias, &vector);
        } else {
            query.push(semantic_floor(alias));
            query.push("::float8 AS semantic, ");
        }
        query.push(format!("{} FROM ", recency_columns(alias, is_message)));
        query.push(if is_message {
            "rh_messages m"
        } else {
            "rh_summaries s"
        });
        query.push(" WHERE ");
        if let Some(filter) = tag_filter {
            filter.push_condition(&mut query, is_message);
        } else {
            query.push("true");
        }
        match kind {
            ArmKind::Fts | ArmKind::FtsExplicit => {
                query.push(" AND ");
                query.push(format!(
                    "{alias}.content_tsv @@ plainto_tsquery('english', "
                ));
                query.push_bind(options.text.clone());
                query.push(")");
            }
            ArmKind::Trigram => push_literal_match(&mut query, &tokens, is_message),
            ArmKind::Regex => {
                query.push(" AND ");
                if is_message {
                    query.push("(m.content ~* ");
                    query.push_bind(options.text.clone());
                    query.push(" OR coalesce(m.tool_result, '') ~* ");
                    query.push_bind(options.text.clone());
                    query.push(")");
                } else {
                    query.push("s.content ~* ");
                    query.push_bind(options.text.clone());
                }
            }
        }
        if kind == ArmKind::Fts {
            if is_message {
                query.push(format!(
                    " AND CASE WHEN m.role = 'tool'
                                THEN length(btrim(coalesce(m.tool_result, '')))
                                ELSE length(btrim(m.content)) END >= {HYBRID_MIN_CONTENT_LEN}"
                ));
            } else {
                query.push(format!(
                    " AND length(btrim(s.content)) >= {HYBRID_MIN_CONTENT_LEN}"
                ));
            }
        }
        push_time_conditions(&mut query, options, alias);
        if let Some(agent) = &options.agent {
            push_agent_condition(&mut query, agent, is_message);
        }
        if kind == ArmKind::Trigram {
            query.push(format!(
                " ORDER BY score DESC, {alias}.created_at DESC LIMIT {pool}"
            ));
        } else {
            query.push(format!(" ORDER BY score DESC LIMIT {pool}"));
        }
        let fetched = if matches!(kind, ArmKind::Trigram | ArmKind::Regex) {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|source| Error::operation("search", source))?;
            sqlx::query("select set_config('statement_timeout', $1, true)")
                .bind(ESCAPE_HATCH_TIMEOUT)
                .execute(&mut *tx)
                .await
                .map_err(|source| Error::operation("search", source))?;
            let rows = query
                .build_query_as::<ArmSqlRow>()
                .fetch_all(&mut *tx)
                .await
                .map_err(|source| Error::operation("search", source))?;
            tx.commit()
                .await
                .map_err(|source| Error::operation("search", source))?;
            rows
        } else {
            query
                .build_query_as::<ArmSqlRow>()
                .fetch_all(&self.pool)
                .await
                .map_err(|source| Error::operation("search", source))?
        };
        fetched
            .into_iter()
            .map(|row| row.into_arm(is_message))
            .collect()
    }

    pub(crate) async fn vector_arm_rows(
        &self,
        options: &SearchOptions,
        tag_filter: Option<&TagFilter>,
        query_vector: &[f32],
        ef_search: u64,
    ) -> Result<Vec<ArmRow>, Error> {
        let vector = HalfVector::from_f32_slice(query_vector);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|source| Error::operation("vector search", source))?;
        sqlx::query_scalar!(
            r#"select set_config('hnsw.ef_search', $1, true) as "ok!""#,
            ef_search.to_string()
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|source| Error::operation("vector search", source))?;
        let context = ArmContext {
            options,
            tag_filter,
        };
        let mut rows = Vec::new();
        if matches!(options.scope, MemoryScope::Messages | MemoryScope::Both) {
            let message_rows = self.vector_table(&context, &vector, true, &mut tx).await?;
            let chunks = self.chunk_arm_rows(&context, &vector, &mut tx).await?;
            rows.extend(merge_chunk_arm(message_rows, chunks));
        }
        if matches!(options.scope, MemoryScope::Summaries | MemoryScope::Both) {
            rows.extend(self.vector_table(&context, &vector, false, &mut tx).await?);
        }
        drop(tx);
        rows.sort_by(|a, b| {
            b.arm_score
                .total_cmp(&a.arm_score)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(rows)
    }

    async fn vector_table(
        &self,
        context: &ArmContext<'_>,
        vector: &HalfVector,
        is_message: bool,
        tx: &mut sqlx::PgConnection,
    ) -> Result<Vec<ArmRow>, Error> {
        let options = context.options;
        let tag_filter = context.tag_filter;
        let alias = if is_message { "m" } else { "s" };
        let table = if is_message {
            "rh_messages m"
        } else {
            "rh_summaries s"
        };
        let distance = format!("{alias}.embedding");
        let mut query = QueryBuilder::new("");
        push_select_prefix(&mut query, is_message, false);
        query.push("coalesce(1.0 - (");
        query.push(distance.clone());
        query.push(" <=> ");
        query.push_bind(vector.clone());
        query.push("), 0.0)::float8 AS score, ");
        push_embedded_semantic(&mut query, &distance, alias, vector);
        query.push(format!(
            "{} FROM {table} WHERE {alias}.embedding IS NOT NULL",
            recency_columns(alias, is_message)
        ));
        if let Some(filter) = tag_filter {
            query.push(" AND ");
            filter.push_condition(&mut query, is_message);
        }
        push_time_conditions(&mut query, options, alias);
        if let Some(agent) = &options.agent {
            push_agent_condition(&mut query, agent, is_message);
        }
        query.push(" ORDER BY ");
        query.push(distance);
        query.push(" <=> ");
        query.push_bind(vector.clone());
        query.push(format!(" LIMIT {}", hybrid_pool(options.limit)));
        let fetched = query
            .build_query_as::<ArmSqlRow>()
            .fetch_all(&mut *tx)
            .await
            .map_err(|source| Error::operation("vector search", source))?;
        fetched
            .into_iter()
            .map(|row| row.into_arm(is_message))
            .collect()
    }

    async fn chunk_arm_rows(
        &self,
        context: &ArmContext<'_>,
        vector: &HalfVector,
        tx: &mut sqlx::PgConnection,
    ) -> Result<Vec<ArmRow>, Error> {
        let mut query = QueryBuilder::new("");
        push_select_prefix(&mut query, true, true);
        query.push("coalesce(1.0 - (c.embedding <=> ");
        query.push_bind(vector.clone());
        query.push("), 0.0)::float8 AS score, ");
        push_embedded_semantic(&mut query, "c.embedding", "m", vector);
        query.push(format!(
            "{} FROM rh_message_chunks c \
             JOIN rh_messages m ON m.id = c.message_id WHERE c.embedding IS NOT NULL",
            recency_columns("m", true)
        ));
        if let Some(filter) = context.tag_filter {
            query.push(" AND ");
            filter.push_condition(&mut query, true);
        }
        push_time_conditions(&mut query, context.options, "m");
        if let Some(agent) = &context.options.agent {
            push_agent_condition(&mut query, agent, true);
        }
        query.push(" ORDER BY c.embedding <=> ");
        query.push_bind(vector.clone());
        query.push(format!(" LIMIT {}", chunk_pool(context.options.limit)));
        let fetched = query
            .build_query_as::<ArmSqlRow>()
            .fetch_all(&mut *tx)
            .await
            .map_err(|source| Error::operation("chunk search", source))?;
        fetched.into_iter().map(|row| row.into_arm(true)).collect()
    }
}

fn merge_chunk_arm(parents: Vec<ArmRow>, chunks: Vec<ArmRow>) -> Vec<ArmRow> {
    let mut merged: BTreeMap<String, ArmRow> = parents
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();
    for chunk in chunks {
        if merged
            .get(&chunk.id)
            .is_none_or(|current| current.arm_score <= chunk.arm_score)
        {
            merged.insert(chunk.id.clone(), chunk);
        }
    }
    merged.into_values().collect()
}
