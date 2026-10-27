#![cfg(test)]

use chrono::{DateTime, Utc};
use kernel::wiki::builder::{ExtractWiki, WikiAction, WikiPatch};
use serde::Deserialize;
use sqlx::PgPool;
use testkit::fakes::{FakeProvider, Script};
use uuid::Uuid;
use wire::gateway::wiki::{WikiHistoryEntry, WikiSlug};
use wire::stream::openai::FinishReason;
use wire::stream::{LlmResponse, LlmResponseKind};

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
        .expect("time")
        .with_timezone(&Utc)
}

pub(crate) fn slug(text: &str) -> WikiSlug {
    WikiSlug::try_from(text.to_owned()).expect("slug")
}

pub(crate) fn patch() -> WikiPatch {
    WikiPatch {
        action: WikiAction::Create,
        slug: slug("acmekit-deploys"),
        title: "AcmeKit deploys".to_owned(),
        aliases: vec![slug("acmekit-rollout")],
        tags: vec!["deploy".to_owned()],
        entities: vec!["project:acmekit".to_owned(), "tool:deploy_script".to_owned()],
        summary: "AcmeKit is deployed blue-green from the release branch; a health check gates the switch.".to_owned(),
        article: Some("## Process\nCut the release branch on Monday, run the deploy script, watch the health check.".to_owned()),
        related: None,
        history_entry: Some(WikiHistoryEntry {
            date: chrono::NaiveDate::from_ymd_opt(2026, 10, 10).expect("history date"),
            title: "Blue-green adopted".to_owned(),
            body: "Decided to deploy blue-green.".to_owned(),
        }),
        kind: None,
    }
}

pub(crate) fn provider(patches: &[WikiPatch]) -> FakeProvider {
    raw_provider(
        &serde_json::to_string(patches).expect("patches"),
        FinishReason::Stop,
    )
}

pub(crate) fn raw_provider(text: &str, finish: FinishReason) -> FakeProvider {
    FakeProvider::new(vec![Script::Chat(LlmResponse {
        r#type: LlmResponseKind::Text,
        content: Some(text.to_owned()),
        tool_calls: None,
        usage: None,
        finish_reason: Some(finish),
    })])
}

pub(crate) async fn seed(pool: &PgPool) -> Result<ExtractWiki, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL rivet.defer_embed_enqueue = 'on'")
        .execute(&mut *tx)
        .await?;
    let conversation_id = Uuid::new_v4();
    let summary_id = Uuid::new_v4();
    sqlx::query("INSERT INTO rh_conversations (id, session_key, agent) VALUES ($1, $2, 'rivet')")
        .bind(conversation_id)
        .bind(format!("cli:{conversation_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO rh_summaries (id, conversation_id, content, kind) VALUES ($1, $2, 'Deploy with a health gate.', 'leaf')")
        .bind(summary_id).bind(conversation_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(ExtractWiki {
        summary_id,
        conversation_id: Some(conversation_id),
    })
}

#[derive(Deserialize)]
pub(crate) struct Fixture {
    pub(crate) scenarios: Vec<Scenario>,
    pub(crate) constants: Constants,
}
#[derive(Deserialize)]
pub(crate) struct Constants {
    #[serde(rename = "WIKI_PIPELINE_VERSION")]
    pub(crate) wiki_pipeline_version: i32,
}
#[derive(Deserialize)]
pub(crate) struct Scenario {
    pub(crate) name: String,
    pub(crate) tables: Tables,
}
#[derive(Deserialize)]
pub(crate) struct Tables {
    #[serde(rename = "ros_summaries")]
    pub(crate) summaries: Option<Vec<Summary>>,
}
#[derive(Deserialize)]
pub(crate) struct Summary {
    pub(crate) id: Uuid,
    pub(crate) conversation_id: Uuid,
    pub(crate) content: String,
    pub(crate) kind: String,
    pub(crate) depth: i32,
    pub(crate) message_count: i32,
    pub(crate) earliest_at: DateTime<Utc>,
    pub(crate) latest_at: DateTime<Utc>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) model: String,
    pub(crate) pipeline_version: i32,
}

pub(crate) fn fixture() -> Fixture {
    let path = testkit::recording("memory-sqlite/summarizer.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("recording"))
        .expect("typed fixture")
}

pub(crate) async fn seed_recording(pool: &PgPool, row: &Summary) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL rivet.defer_embed_enqueue = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO rh_conversations (id, session_key, agent) VALUES ($1, $2, 'rivet') ON CONFLICT DO NOTHING")
        .bind(row.conversation_id).bind(format!("cli:{}", row.conversation_id)).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO rh_summaries (id, conversation_id, content, kind, depth, message_count, earliest_at, latest_at, created_at, model, pipeline_version) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(row.id).bind(row.conversation_id).bind(&row.content).bind(&row.kind).bind(row.depth).bind(row.message_count)
        .bind(row.earliest_at).bind(row.latest_at).bind(row.created_at).bind(&row.model).bind(row.pipeline_version)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
