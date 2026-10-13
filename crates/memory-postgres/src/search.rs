use crate::error::Error;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::SearchMode;
use kernel::memory::TagLiteral;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use uuid::Uuid;
use wire::gateway::memory::MemoryFallback;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;
use wire::memory::MemorySearchResult;
use wire::message::Role;

const HYBRID_RRF_K: f64 = 20.0;
const SUMMARY_FUSION_BONUS: f64 = 1.3;
const GATE_FRACTION: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchOptions {
    pub(crate) text: String,
    pub(crate) mode: SearchMode,
    pub(crate) scope: MemoryScope,
    pub(crate) limit: u32,
    pub(crate) agent: Option<String>,
    pub(crate) tag: Option<TagLiteral>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub degraded: Option<String>,
    pub fallback: Option<MemoryFallback>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub source: MemoryHitSource,
    pub content: String,
    pub role: Option<Role>,
    pub agent: Option<String>,
    pub kind: Option<String>,
    pub tool_name: Option<String>,
    pub conversation_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub score: f64,
    pub fallback: Option<MemoryFallback>,
}

impl SearchHit {
    #[must_use]
    pub fn into_port_result(self) -> MemorySearchResult {
        MemorySearchResult {
            id: self.id,
            source: self.source,
            content: self.content,
            role: self.role,
            agent: self.agent,
            relevance_score: self.score,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ArmRow {
    pub(crate) id: String,
    pub(crate) source: MemoryHitSource,
    pub(crate) content: String,
    pub(crate) role: Option<Role>,
    pub(crate) agent: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) conversation_id: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) arm_score: f64,
    pub(crate) boost: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArmKind {
    Fts,
    FtsExplicit,
    Trigram,
    Regex,
}

#[expect(
    clippy::expect_used,
    reason = "the literal-scan pattern is a constant; the search tests match it"
)]
pub(crate) fn literal_scan(text: &str) -> bool {
    static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\w[./:_@]\w|\d{1,3}(?:\.\d{1,3}){2,}|[a-z]\d|\d[a-z]")
            .expect("the literal-scan pattern is a constant")
    });
    LITERAL.is_match(text)
}

#[expect(
    clippy::expect_used,
    reason = "the trigram-fallback pattern is a constant; the search tests match it"
)]
fn should_trigram_fallback(text: &str) -> bool {
    static FALLBACK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\S[./:-]\S").expect("the trigram-fallback pattern is a constant")
    });
    literal_scan(text) || FALLBACK.is_match(text)
}

#[expect(clippy::cast_precision_loss, reason = "coarse ranking proxy")]
fn semantic_proxy(content: &str) -> f64 {
    (content.chars().count() as f64 / 1000.0).min(1.0)
}

struct FusedEntry {
    hit: SearchHit,
    arm_count: u8,
}

fn fuse(fts: &[ArmRow], trigram: Option<&[ArmRow]>) -> Vec<FusedEntry> {
    let arms = [Some(fts), trigram];
    let mut entries: HashMap<String, (ArmRow, f64, u8)> = HashMap::new();
    for (arm_number, arm) in (1u8..).zip(arms.into_iter().flatten()) {
        for (rank, row) in (0u32..).zip(arm.iter()) {
            let contribution = 1.0 / (HYBRID_RRF_K + f64::from(rank) + 1.0);
            let entry = entries
                .entry(row.id.clone())
                .or_insert_with(|| (row.clone(), 0.0, 0));
            entry.1 += contribution;
            entry.2 = entry.2.max(arm_number);
        }
    }
    let mut fused: Vec<FusedEntry> = entries
        .into_values()
        .map(|(row, rrf, arm_count)| {
            let bonus = if row.source == MemoryHitSource::Summary {
                SUMMARY_FUSION_BONUS
            } else {
                1.0
            };
            let score = rrf * (1.0 + row.boost) * bonus;
            FusedEntry {
                arm_count,
                hit: SearchHit {
                    id: row.id,
                    source: row.source,
                    content: row.content,
                    role: row.role,
                    agent: row.agent,
                    kind: row.kind,
                    tool_name: row.tool_name,
                    conversation_id: row.conversation_id,
                    created_at: row.created_at,
                    score,
                    fallback: None,
                },
            }
        })
        .collect();
    fused.sort_by(|a, b| b.hit.score.total_cmp(&a.hit.score));
    fused
}

pub(crate) struct TagFilter {
    pub(crate) key: String,
    pub(crate) value: String,
}

impl From<&TagLiteral> for TagFilter {
    fn from(literal: &TagLiteral) -> Self {
        Self {
            key: literal.key.clone(),
            value: literal.value.clone(),
        }
    }
}

#[expect(clippy::suboptimal_flops, reason = "mirrors recorded scoring")]
fn composite_score(row: &ArmRow) -> f64 {
    row.arm_score * 0.3 + semantic_proxy(&row.content) * 0.3 + row.boost
}

fn finish_rows(rows: Vec<ArmRow>, limit: u32, score: fn(&ArmRow) -> f64) -> Vec<SearchHit> {
    let mut hits: Vec<SearchHit> = rows
        .into_iter()
        .map(|row| {
            let computed = score(&row);
            SearchHit {
                id: row.id,
                source: row.source,
                content: row.content,
                role: row.role,
                agent: row.agent,
                kind: row.kind,
                tool_name: row.tool_name,
                conversation_id: row.conversation_id,
                created_at: row.created_at,
                score: computed,
                fallback: None,
            }
        })
        .collect();
    hits.truncate(limit.max(1) as usize);
    hits
}

impl crate::PostgresMemory {
    pub(crate) async fn search_full(
        &self,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, Error> {
        Ok(self.search_outcome(options).await?.hits)
    }

    pub(crate) async fn search_outcome(
        &self,
        options: &SearchOptions,
    ) -> Result<SearchOutcome, Error> {
        let tag_filter = options.tag.as_ref().map(TagFilter::from);
        match options.mode {
            SearchMode::Hybrid => self.hybrid_search(options, tag_filter.as_ref()).await,
            SearchMode::Vector | SearchMode::Fts | SearchMode::Trigram | SearchMode::Regex => {
                let kind = match options.mode {
                    SearchMode::Trigram => ArmKind::Trigram,
                    SearchMode::Regex => ArmKind::Regex,
                    SearchMode::Fts | SearchMode::Hybrid | SearchMode::Vector => {
                        ArmKind::FtsExplicit
                    }
                };
                let rows = self.arm_rows(options, kind, tag_filter.as_ref()).await?;
                let mut hits = if rows.is_empty()
                    && options.mode == SearchMode::Fts
                    && should_trigram_fallback(&options.text)
                {
                    let recovered = self
                        .arm_rows(options, ArmKind::Trigram, tag_filter.as_ref())
                        .await?;
                    let recovered_nonempty = !recovered.is_empty();
                    let mut converted = finish_rows(recovered, options.limit, composite_score);
                    if recovered_nonempty {
                        for hit in &mut converted {
                            hit.fallback = Some(MemoryFallback::Trigram);
                        }
                    }
                    return Ok(SearchOutcome {
                        hits: converted,
                        degraded: None,
                        fallback: None,
                    });
                } else {
                    finish_rows(rows, options.limit, composite_score)
                };
                hits.truncate(options.limit.max(1) as usize);
                self.bump_access(&hits).await?;
                Ok(SearchOutcome {
                    hits,
                    degraded: None,
                    fallback: None,
                })
            }
        }
    }

    async fn hybrid_search(
        &self,
        options: &SearchOptions,
        tag_filter: Option<&TagFilter>,
    ) -> Result<SearchOutcome, Error> {
        let literal = literal_scan(&options.text);
        let fts_rows = self.arm_rows(options, ArmKind::Fts, tag_filter).await?;
        let trigram_rows = if literal {
            Some(self.arm_rows(options, ArmKind::Trigram, tag_filter).await?)
        } else {
            None
        };

        let mut fallback = None;
        let trigram_rows = match trigram_rows {
            Some(rows) => Some(rows),
            None => {
                if fts_rows.is_empty() && should_trigram_fallback(&options.text) {
                    let recovered = self.arm_rows(options, ArmKind::Trigram, tag_filter).await?;
                    if !recovered.is_empty() {
                        fallback = Some(MemoryFallback::Trigram);
                    }
                    Some(recovered)
                } else {
                    None
                }
            }
        };

        let fused = fuse(&fts_rows, trigram_rows.as_deref());
        let top = fused.first().map_or(0.0, |entry| entry.hit.score);
        let mut hits: Vec<SearchHit> = fused
            .into_iter()
            .filter(|entry| entry.arm_count >= 2 || entry.hit.score >= top * GATE_FRACTION)
            .map(|entry| entry.hit)
            .collect();
        hits.truncate(options.limit.max(1) as usize);
        self.bump_access(&hits).await?;
        Ok(SearchOutcome {
            hits,
            degraded: None,
            fallback,
        })
    }

    async fn bump_access(&self, hits: &[SearchHit]) -> Result<(), Error> {
        let messages: Vec<Uuid> = hit_uuids(hits, MemoryHitSource::Message)?;
        let summaries: Vec<Uuid> = hit_uuids(hits, MemoryHitSource::Summary)?;
        if !messages.is_empty() {
            sqlx::query!(
                "update rh_messages set access_count = access_count + 1,
                        last_accessed_at = now() where id = any($1)",
                &messages
            )
            .execute(&self.pool)
            .await
            .map_err(|source| Error::operation("access bump", source))?;
        }
        if !summaries.is_empty() {
            sqlx::query!(
                "update rh_summaries set access_count = access_count + 1,
                        last_accessed_at = now() where id = any($1)",
                &summaries
            )
            .execute(&self.pool)
            .await
            .map_err(|source| Error::operation("access bump", source))?;
        }
        Ok(())
    }
}

fn hit_uuids(hits: &[SearchHit], source: MemoryHitSource) -> Result<Vec<Uuid>, Error> {
    hits.iter()
        .filter(|hit| hit.source == source)
        .map(|hit| {
            Uuid::parse_str(&hit.id).map_err(|error| {
                Error::protocol(format!("search hit id {} is not a uuid: {error}", hit.id))
            })
        })
        .collect()
}
