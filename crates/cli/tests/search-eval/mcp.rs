use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::time::Instant;

use kernel::memory::SearchMode;
use rmcp::ServiceExt as _;
use rmcp::model::CallToolRequestParams;
use rmcp::model::ClientConfig;
use rmcp::service::QuitReason;
use rmcp::service::RoleClient;
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use serde::Serialize;
use wire::gateway::memory::MemoryScope;

use crate::Failure;
use crate::score::Scored;
use crate::set::EvalQuery;
use crate::set::QuerySet;
use crate::shown::Ranking;
use crate::shown::Reader;
use crate::store::Opened;

const TOOL: &str = "memory_search";
const PLACEHOLDER: &str = "{sqlite}";
const WARM_UP_LIMIT: u32 = 1;

#[derive(Debug, Serialize)]
struct Arguments<'query> {
    query: &'query str,
    mode: SearchMode,
    scope: MemoryScope,
    limit: u32,
}

struct Server {
    service: RunningService<RoleClient, ClientConfig>,
    reader: Reader,
}

pub(crate) async fn evaluate(
    argv: &[OsString],
    opened: Option<&Opened>,
    set: &QuerySet,
) -> Result<Vec<Scored>, Failure> {
    let contents = match opened {
        Some(opened) => crate::store::contents(&opened.store, relevant(set)).await?,
        None => BTreeMap::new(),
    };
    let database = opened.map(|opened| opened.path.as_path());
    let server = Server::spawn(argv, database).await?;
    let scored = server.score(set, &contents).await;
    let closed = server.close().await;
    let scored = scored?;
    closed?;
    Ok(scored)
}

fn relevant(set: &QuerySet) -> Vec<String> {
    let mut ids: Vec<String> = set
        .queries
        .iter()
        .flat_map(|query| query.relevant.iter().cloned())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn placed(arg: &OsStr, database: Option<&Path>) -> OsString {
    match (arg.to_str(), database) {
        (Some(text), Some(path)) if text.contains(PLACEHOLDER) => {
            let mut placed = OsString::new();
            for (index, part) in text.split(PLACEHOLDER).enumerate() {
                if index > 0 {
                    placed.push(path);
                }
                placed.push(part);
            }
            placed
        }
        _ => arg.to_owned(),
    }
}

impl Server {
    async fn spawn(argv: &[OsString], database: Option<&Path>) -> Result<Self, Failure> {
        let (program, rest) = argv.split_first().ok_or(Failure::NoServer)?;
        let mut command = tokio::process::Command::new(placed(program, database));
        command.args(rest.iter().map(|arg| placed(arg, database)));
        let transport = TokioChildProcess::new(command).map_err(Failure::Spawn)?;
        let service = ClientConfig::default()
            .serve(transport)
            .await
            .map_err(|error| Failure::Initialize(Box::new(error)))?;
        let tools = service.list_all_tools().await?;
        if !tools.iter().any(|tool| tool.name == TOOL) {
            let offered: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
            return Err(Failure::NoTool {
                offered: offered.join(", "),
            });
        }
        Ok(Self {
            service,
            reader: Reader::new()?,
        })
    }

    async fn score(
        &self,
        set: &QuerySet,
        contents: &BTreeMap<String, String>,
    ) -> Result<Vec<Scored>, Failure> {
        if let Some(first) = set.queries.first() {
            self.search(first, WARM_UP_LIMIT).await?;
        }
        let mut scored = Vec::with_capacity(set.queries.len());
        for query in &set.queries {
            let started = Instant::now();
            let ranking = self.search(query, set.limit.get()).await?;
            scored.push(Scored::new(query, &ranking, contents, started.elapsed())?);
        }
        Ok(scored)
    }

    async fn search(&self, query: &EvalQuery, limit: u32) -> Result<Ranking, Failure> {
        let arguments = Arguments {
            query: &query.query,
            mode: query.mode,
            scope: MemoryScope::Both,
            limit,
        };
        let mut request = CallToolRequestParams::new(TOOL);
        request.arguments = Some(
            serde_json::from_str(&serde_json::to_string(&arguments).map_err(Failure::Arguments)?)
                .map_err(Failure::Arguments)?,
        );
        let result = self.service.call_tool(request).await?;
        let text: Vec<&str> = result
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|text| text.text.as_str()))
            .collect();
        let text = text.join("\n");
        if result.is_error == Some(true) {
            return Err(Failure::Tool {
                id: query.id.clone(),
                text,
            });
        }
        Ok(self.reader.read(&text))
    }

    async fn close(self) -> Result<(), Failure> {
        match self.service.cancel().await? {
            QuitReason::Closed | QuitReason::Cancelled => Ok(()),
            QuitReason::JoinError(error) => Err(Failure::Join(error)),
            other => Err(Failure::Quit(format!("{other:?}"))),
        }
    }
}
