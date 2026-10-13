use crate::error::Error;
use crate::search::ArmKind;
use crate::search::ArmRow;
use crate::search::SearchOptions;
use crate::search::TagFilter;
use chrono::Utc;
use std::str::FromStr as _;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;
use wire::message::Role;

const HYBRID_POOL_MIN: usize = 50;
const HYBRID_POOL_MAX: usize = 100;
const MIN_CONTENT_LEN: i64 = 40;

fn hybrid_pool(limit: u32) -> i64 {
    let pool = usize::try_from(limit).unwrap_or(HYBRID_POOL_MAX) * 3;
    let clamped = pool.clamp(HYBRID_POOL_MIN, HYBRID_POOL_MAX);
    i64::try_from(clamped)
        .unwrap_or_else(|_| i64::from(u32::try_from(HYBRID_POOL_MAX).unwrap_or(100)))
}

fn boost_expr(alias: &str, is_message: bool) -> String {
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
        "(EXP(-0.05 * EXTRACT(EPOCH FROM (now() - {alias}.created_at)) / 86400.0)
            * (1.0 + 0.01 * LEAST(COALESCE({alias}.access_count, 0), 25))) * 0.3
         + {importance} * 0.1"
    )
}

fn role_of(raw: Option<String>) -> Option<Role> {
    raw.and_then(|role| Role::from_str(&role).ok())
}

impl TagFilter {
    pub(crate) fn push_condition(
        &self,
        query: &mut sqlx::QueryBuilder<sqlx::Postgres>,
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

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArmSqlRow {
    id: String,
    content: String,
    role_or_kind: Option<String>,
    agent: Option<String>,
    tool_name: Option<String>,
    conversation_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
    score: f64,
    boost: f64,
}

impl ArmSqlRow {
    fn into_arm(self, is_message: bool) -> ArmRow {
        let (role, kind) = if is_message {
            (role_of(self.role_or_kind), None)
        } else {
            (None, self.role_or_kind)
        };
        ArmRow {
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
            created_at: self.created_at,
            arm_score: self.score,
            boost: self.boost,
        }
    }
}

impl crate::PostgresMemory {
    pub(crate) async fn arm_rows(
        &self,
        options: &SearchOptions,
        kind: ArmKind,
        tag_filter: Option<&TagFilter>,
    ) -> Result<Vec<ArmRow>, Error> {
        let pool = hybrid_pool(options.limit);
        let mut rows = Vec::new();
        if matches!(options.scope, MemoryScope::Messages | MemoryScope::Both) {
            rows.extend(
                self.arm_table(options, kind, pool, tag_filter, true)
                    .await?,
            );
        }
        if matches!(options.scope, MemoryScope::Summaries | MemoryScope::Both) {
            rows.extend(
                self.arm_table(options, kind, pool, tag_filter, false)
                    .await?,
            );
        }
        rows.sort_by(|a, b| {
            b.arm_score
                .partial_cmp(&a.arm_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(rows)
    }

    #[expect(clippy::too_many_lines, reason = "one SQL builder covers both tables")]
    async fn arm_table(
        &self,
        options: &SearchOptions,
        kind: ArmKind,
        pool: i64,
        tag_filter: Option<&TagFilter>,
        is_message: bool,
    ) -> Result<Vec<ArmRow>, Error> {
        let alias = if is_message { "m" } else { "s" };
        let mut query = sqlx::QueryBuilder::new("SELECT ");
        query.push(format!("{alias}.id::text, {alias}.content, "));
        if is_message {
            query.push("m.role AS role_or_kind, m.agent, m.tool_name, m.conversation_id::text AS conversation_id, ");
        } else {
            query.push("s.kind AS role_or_kind, NULL AS agent, NULL AS tool_name, NULL AS conversation_id, ");
        }
        query.push(format!("{alias}.created_at, "));
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
            ArmKind::Trigram => {
                if is_message {
                    query.push("GREATEST(similarity(m.content, ");
                    query.push_bind(options.text.clone());
                    query.push("), similarity(coalesce(m.tool_result, ''), ");
                    query.push_bind(options.text.clone());
                    query.push("))::float8");
                } else {
                    query.push("similarity(s.content, ");
                    query.push_bind(options.text.clone());
                    query.push(")::float8");
                }
            }
            ArmKind::Regex => {
                query.push("1.0::float8");
            }
        }
        query.push(" AS score, ");
        query.push(format!(
            "({})::float8 AS boost FROM ",
            boost_expr(alias, is_message)
        ));
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
            ArmKind::Trigram => {
                query.push(" AND (similarity(");
                query.push(format!("{alias}.content, "));
                query.push_bind(options.text.clone());
                query.push(") > 0.3");
                if is_message {
                    query.push(" OR similarity(coalesce(m.tool_result, ''), ");
                    query.push_bind(options.text.clone());
                    query.push(") > 0.3");
                }
                query.push(")");
            }
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
                                ELSE length(btrim(m.content)) END >= {MIN_CONTENT_LEN}"
                ));
            } else {
                query.push(format!(
                    " AND length(btrim(s.content)) >= {MIN_CONTENT_LEN}"
                ));
            }
        }
        if let Some(agent) = &options.agent {
            if is_message {
                query.push(" AND m.agent = ");
                query.push_bind(agent.clone());
            } else {
                query.push(
                    " AND EXISTS (SELECT 1 FROM rh_conversations c \
                     WHERE c.id = s.conversation_id AND c.agent = ",
                );
                query.push_bind(agent.clone());
                query.push(")");
            }
        }
        query.push(format!(" ORDER BY score DESC LIMIT {pool}"));
        let fetched = query
            .build_query_as::<ArmSqlRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("search", source))?;
        Ok(fetched
            .into_iter()
            .map(|row| row.into_arm(is_message))
            .collect())
    }
}
