use crate::{Error, error::required, json, memory::limit};
use kernel::memory::{
    MemoryStore, TagAdd, TagCountsQuery, TagDecision, TagEntity, TagId, TagListQuery, TagLookup,
    TagTaxonomyDecision, TagTaxonomyQuery, TagTaxonomyUpsert,
};
use rmcp::model::CallToolResult;
use serde::Deserialize;
use uuid::Uuid;
use wire::gateway::memory::{TagEntityType, TagEntryPair, TagState, TagTaxonomyMerge};

#[derive(schemars::JsonSchema, serde::Serialize, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    List,
    #[default]
    Pending,
    Counts,
    Decide,
    Add,
    Lookup,
    Taxonomy,
    TaxonomyUpsert,
    TaxonomyDecide,
    TaxonomyMerge,
}

#[derive(schemars::JsonSchema, Debug, Deserialize)]
#[schemars(!deny_unknown_fields)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[serde(default)]
    #[schemars(description = "What to do (default: pending)")]
    action: Action,
    #[schemars(description = "add with session_key: narrow to one agent", transform = crate::catalog::optional_non_null)]
    agent: Option<String>,
    #[schemars(description = "taxonomy_upsert: replaces the alias list ([] clears it; omit to keep it)", transform = crate::catalog::optional_non_null)]
    aliases: Option<Vec<String>>,
    #[schemars(description = "decide / add: who decided (audit column)", transform = crate::catalog::optional_non_null)]
    decided_by: Option<String>,
    #[schemars(description = "Display casing (add / taxonomy_upsert)", transform = crate::catalog::optional_non_null)]
    display: Option<String>,
    #[schemars(description = "Conversation or summary id (list / add)", with = "Option<String>", transform = crate::catalog::optional_non_null)]
    entity_id: Option<Uuid>,
    #[schemars(description = "list / add", transform = crate::catalog::optional_non_null)]
    entity_type: Option<TagEntityType>,
    #[schemars(description = "taxonomy_decide: [{key, value}]", length(max = 500), transform = crate::catalog::optional_non_null)]
    entries: Option<Vec<TagEntryPair>>,
    #[schemars(description = "taxonomy_merge: value to fold away", transform = crate::catalog::optional_non_null)]
    from: Option<String>,
    #[schemars(description = "Tag ids (decide)", length(max = 1000), with = "Option<Vec<String>>", transform = crate::catalog::optional_non_null)]
    ids: Option<Vec<Uuid>>,
    #[schemars(description = "taxonomy_merge: value that survives", transform = crate::catalog::optional_non_null)]
    into: Option<String>,
    #[schemars(description = "Tag key, e.g. project, topic", transform = crate::catalog::optional_non_null)]
    key: Option<String>,
    #[schemars(range(min = 1, max = 1000), transform = crate::catalog::optional_non_null)]
    limit: Option<u32>,
    #[schemars(description = "taxonomy_upsert: nest under this value (null clears the parent)")]
    parent_value: Option<String>,
    #[schemars(transform = crate::catalog::optional_non_null)]
    reason: Option<String>,
    #[schemars(description = "add: tag the conversation captured under this session key", transform = crate::catalog::optional_non_null)]
    session_key: Option<String>,
    #[schemars(description = "lookup", length(max = 500), transform = crate::catalog::optional_non_null)]
    session_keys: Option<Vec<String>>,
    #[schemars(description = "decide / taxonomy_decide: accepted or rejected. list / taxonomy: filter.", transform = crate::catalog::optional_non_null)]
    state: Option<TagState>,
    #[schemars(description = "key:value literal (add)", transform = crate::catalog::optional_non_null)]
    tag: Option<String>,
    #[schemars(description = "Tag value", transform = crate::catalog::optional_non_null)]
    value: Option<String>,
}

impl crate::catalog::ToolInput for Args {
    const NAME: &'static str = "memory_tags";
    const DESCRIPTION: &'static str = "Read key:value tags on sessions and summaries (session tagging). Actions: pending (suggestions awaiting review; the default), list (tags on an entity or by key+value), counts (accepted usage per tag), lookup (tags for session_keys), taxonomy (the vocabulary). This surface is READ-ONLY: decide, add and the taxonomy edits are refused (set RIVETOS_MCP_ENABLE_MEMORY_WRITE=1, or review in the hub under Memory → Tags). Rejecting keeps the row so the tagger never re-proposes it. Use tag=key:value on memory_search / memory_browse to filter by an accepted tag.";
    fn annotations() -> Option<rmcp::model::ToolAnnotations> {
        Some(rmcp::model::ToolAnnotations::from_raw(
            None,
            Some(true),
            None,
            Some(true),
            None,
        ))
    }
}

pub async fn call(
    store: &dyn MemoryStore,
    write: bool,
    args: Args,
) -> Result<CallToolResult, Error> {
    authorize(write, &args.action)?;
    let count = limit(args.limit, 100, 1000)?;
    match args.action {
        Action::Pending => json(&store.pending_tags(count).await?),
        Action::List => json(
            &store
                .list_tags(TagListQuery {
                    entity_type: args.entity_type,
                    entity_id: args.entity_id,
                    key: args.key,
                    value: args.value,
                    states: args.state.into_iter().collect(),
                    limit: count,
                })
                .await?,
        ),
        Action::Counts => json(
            &store
                .tag_counts(TagCountsQuery {
                    key: args.key,
                    limit: count,
                })
                .await?,
        ),
        Action::Lookup => {
            let session_keys = required(args.session_keys, "session_keys")?;
            bounded(&session_keys, 500)?;
            json(
                &store
                    .lookup_tags(TagLookup {
                        session_keys,
                        states: args.state.into_iter().collect(),
                    })
                    .await?,
            )
        }
        Action::Taxonomy => json(
            &store
                .list_tag_taxonomy(TagTaxonomyQuery {
                    key: args.key,
                    state: args.state,
                    limit: count,
                })
                .await?,
        ),
        Action::Decide => decide(store, args).await,
        Action::Add => add(store, args).await,
        Action::TaxonomyUpsert => upsert(store, args).await,
        Action::TaxonomyDecide => {
            let entries = required(args.entries, "entries")?;
            bounded(&entries, 500)?;
            json(
                &store
                    .decide_tag_taxonomy(TagTaxonomyDecision {
                        entries,
                        decision: decision(args.state)?,
                        decided_by: args.decided_by,
                    })
                    .await?,
            )
        }
        Action::TaxonomyMerge => json(
            &store
                .merge_tag_taxonomy(TagTaxonomyMerge {
                    key: required(args.key, "key")?,
                    from: required(args.from, "from")?,
                    into: required(args.into, "into")?,
                })
                .await?,
        ),
    }
}

fn decision(state: Option<TagState>) -> Result<TagDecision, Error> {
    match required(state, "state")? {
        TagState::Accepted => Ok(TagDecision::Accept),
        TagState::Rejected => Ok(TagDecision::Reject),
        TagState::Suggested => Err(Error::Invalid("state must be accepted or rejected".into())),
    }
}

fn bounded<T>(values: &[T], maximum: usize) -> Result<(), Error> {
    if values.is_empty() || values.len() > maximum {
        return Err(Error::Invalid(format!("expected 1..={maximum} items")));
    }
    Ok(())
}

async fn add(store: &dyn MemoryStore, args: Args) -> Result<CallToolResult, Error> {
    let entity = match (args.entity_id, args.session_key) {
        (Some(id), None) => TagEntity::Id(id),
        (None, Some(key)) => TagEntity::SessionKey(key),
        _ => {
            return Err(Error::Invalid(
                "exactly one of entity_id and session_key is required".into(),
            ));
        }
    };
    let tag = required(args.tag, "tag")?
        .parse()
        .map_err(|error: kernel::memory::TagLiteralError| Error::Invalid(error.to_string()))?;
    json(
        &store
            .add_tag(TagAdd {
                entity_type: required(args.entity_type, "entity_type")?,
                entity,
                agent: args.agent,
                tag,
                display: args.display,
                reason: args.reason,
                decided_by: args.decided_by,
            })
            .await?,
    )
}

async fn upsert(store: &dyn MemoryStore, args: Args) -> Result<CallToolResult, Error> {
    let aliases = required(
        args.aliases,
        "aliases (explicit replacement required by MemoryStore)",
    )?;
    json(
        &store
            .upsert_tag_taxonomy(TagTaxonomyUpsert {
                key: required(args.key, "key")?,
                value: required(args.value, "value")?,
                display: args.display,
                parent_value: args.parent_value,
                aliases,
                reason: args.reason,
            })
            .await?,
    )
}

async fn decide(store: &dyn MemoryStore, args: Args) -> Result<CallToolResult, Error> {
    let ids = required(args.ids, "ids")?;
    bounded(&ids, 1000)?;
    json(
        &store
            .decide_tags(
                ids.into_iter().map(TagId::from_uuid).collect(),
                decision(args.state)?,
                args.decided_by,
            )
            .await?,
    )
}

fn authorize(write: bool, action: &Action) -> Result<(), Error> {
    if !write
        && matches!(
            action,
            Action::Decide
                | Action::Add
                | Action::TaxonomyUpsert
                | Action::TaxonomyDecide
                | Action::TaxonomyMerge
        )
    {
        return Err(Error::Refused("memory tag writes are disabled"));
    }
    Ok(())
}
