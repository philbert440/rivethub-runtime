pub mod builder;
pub mod consolidate;
pub mod entity;
pub mod page;
pub mod route;

use wire::gateway::wiki::WikiPageResponse;

pub const WIKI_LIMIT_MAX: u32 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiFilter {
    pub query: Option<String>,
    pub tag: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiLookup {
    Page(Box<WikiPageResponse>),
    Redirect(String),
    Missing,
}
