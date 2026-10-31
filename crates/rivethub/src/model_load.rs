use std::time::Duration;

use async_trait::async_trait;
use config::Environment;
use kernel::BoxError;
use kernel::model::ModelLoad;
use kernel::model::ModelLoadProbe;

const METRICS_URL: &str = "RIVETHUB_WIKI_BACKFILL_METRICS_URL";
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct VllmMetrics {
    client: reqwest::Client,
    url: String,
}

impl VllmMetrics {
    pub fn from_environment(env: &Environment) -> Result<Option<Self>, reqwest::Error> {
        let Some(url) = env
            .text(METRICS_URL)
            .map(str::trim)
            .filter(|url| !url.is_empty())
        else {
            return Ok(None);
        };
        let client = reqwest::Client::builder().timeout(TIMEOUT).build()?;
        Ok(Some(Self {
            client,
            url: url.to_owned(),
        }))
    }
}

#[async_trait]
impl ModelLoadProbe for VllmMetrics {
    async fn load(&self) -> Result<ModelLoad, BoxError> {
        let text = self
            .client
            .get(&self.url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(ModelLoad {
            running: gauge(&text, "vllm:num_requests_running")?,
            waiting: gauge(&text, "vllm:num_requests_waiting")?,
        })
    }
}

fn gauge(text: &str, name: &str) -> Result<u32, BoxError> {
    let mut total = None;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        if !rest.starts_with('{') && !rest.starts_with(' ') {
            continue;
        }
        let value: f64 = line
            .rsplit(' ')
            .next()
            .ok_or_else(|| format!("{name} has no value"))?
            .parse()?;
        total = Some(total.map_or(value, |sum: f64| sum + value));
    }
    let total = total.ok_or_else(|| format!("{name} is not in the metrics"))?;
    let count = format!("{:.0}", total.max(0.0));
    Ok(count.parse()?)
}
