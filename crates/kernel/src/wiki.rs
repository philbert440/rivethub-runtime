pub mod builder;
pub mod consolidate;
pub mod entity;
pub mod route;

use wire::gateway::wiki::WikiPageResponse;
use wire::gateway::wiki::WikiQuery;

pub const WIKI_LIMIT_MAX: u32 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiFilter {
    pub query: Option<String>,
    pub tag: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl From<WikiQuery> for WikiFilter {
    fn from(query: WikiQuery) -> Self {
        #[expect(
            clippy::manual_unwrap_or_default,
            reason = "CONTRACT B1: an absent offset selects the first page; serde rejects invalid offsets"
        )]
        let offset = match query.offset {
            Some(offset) => offset,
            None => 0,
        };
        Self {
            query: query.q.filter(|text| !text.trim().is_empty()),
            tag: query.tag,
            limit: match query.limit {
                Some(limit) => limit.clamp(1, WIKI_LIMIT_MAX),
                None => 50,
            },
            offset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiLookup {
    Page(Box<WikiPageResponse>),
    Redirect(String),
    Missing,
}
