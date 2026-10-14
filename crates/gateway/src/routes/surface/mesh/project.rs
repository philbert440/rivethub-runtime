use serde::Deserialize;
use url::Url;
use wire::JsonObject;
use wire::gateway::devices::MeshOverview;
use wire::gateway::devices::MeshOverviewNode;
use wire::mesh::MeshFile;
use wire::mesh::MeshNode;

use crate::error::ApiError;

const DEFAULT_GATEWAY_PORT: u16 = 5174;
const GATEWAY_CAPABILITY: &str = "den";

#[derive(Debug, Deserialize)]
struct GatewayUrl {
    #[serde(rename = "denUrl")]
    url: String,
}

#[derive(Debug, Deserialize)]
struct GatewayPort {
    #[serde(rename = "denPort")]
    port: PortValue,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PortValue {
    Number(u16),
    Wide(i64),
    Float(f64),
    Text(String),
    Bool(bool),
}

pub(super) fn project(file: &MeshFile) -> Result<MeshOverview, ApiError> {
    let mut nodes = Vec::new();
    for node in file.nodes.values() {
        let Some(name) = usable(&node.name) else {
            continue;
        };
        let Some(url) = node_url(node)? else {
            continue;
        };
        if node.id.is_empty() {
            continue;
        }
        nodes.push(MeshOverviewNode {
            id: node.id.clone(),
            name: name.to_owned(),
            url,
            online: false,
            sessions: None,
            latest: None,
        });
    }
    Ok(MeshOverview {
        updated_at: file.updated_at,
        nodes,
    })
}

fn node_url(node: &MeshNode) -> Result<Option<String>, ApiError> {
    if let Some(metadata) = &node.metadata {
        if let Some(raw) = optional_field::<GatewayUrl>(metadata)?
            && let Some(url) = http_url(&raw.url)
        {
            return Ok(Some(url));
        }
        if let Some(value) = optional_field::<GatewayPort>(metadata)?
            && let Some(port) = gateway_port(&value.port)
            && let Some(host) = usable(&node.host)
        {
            return Ok(Some(format!("http://{host}:{port}")));
        }
    }
    if node
        .capabilities
        .iter()
        .any(|cap| cap == GATEWAY_CAPABILITY)
        && let Some(host) = usable(&node.host)
    {
        return Ok(Some(format!("http://{host}:{DEFAULT_GATEWAY_PORT}")));
    }
    Ok(None)
}

fn optional_field<T: serde::de::DeserializeOwned>(
    metadata: &JsonObject,
) -> Result<Option<T>, ApiError> {
    let bytes = serde_json::to_vec(metadata).map_err(serialize_error)?;
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_data() => Ok(None),
        Err(error) => Err(serialize_error(error)),
    }
}

fn serialize_error(error: serde_json::Error) -> ApiError {
    tracing::error!("mesh metadata: {error}");
    ApiError::Serialize(error)
}

fn http_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match Url::parse(trimmed) {
        Ok(parsed) if parsed.scheme() == "http" || parsed.scheme() == "https" => {
            Some(trimmed.to_owned())
        }
        Ok(parsed) => {
            tracing::warn!(
                scheme = parsed.scheme(),
                "ignoring gateway url with a non-http scheme"
            );
            None
        }
        Err(error) => {
            tracing::warn!("ignoring unparseable gateway url: {error}");
            None
        }
    }
}

fn gateway_port(value: &PortValue) -> Option<u16> {
    match value {
        PortValue::Number(port) => nonzero(*port),
        PortValue::Wide(port) => match u16::try_from(*port) {
            Ok(port) => nonzero(port),
            Err(error) => {
                tracing::debug!("gateway port out of range: {error}");
                None
            }
        },
        PortValue::Float(value) => {
            tracing::debug!(port = value, "ignoring a fractional gateway port");
            None
        }
        PortValue::Bool(value) => {
            tracing::debug!(port = value, "ignoring a boolean gateway port");
            None
        }
        PortValue::Text(text) => match text.trim().parse::<u16>() {
            Ok(port) => nonzero(port),
            Err(error) => {
                tracing::debug!("gateway port text: {error}");
                None
            }
        },
    }
}

fn nonzero(port: u16) -> Option<u16> {
    if port == 0 { None } else { Some(port) }
}

fn usable(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::project;
    use chrono::DateTime;
    use chrono::Utc;
    use serde::Serialize;
    use wire::Extra;
    use wire::JsonObject;
    use wire::mesh::MeshFile;
    use wire::mesh::MeshNode;
    use wire::mesh::MeshNodeStatus;

    #[derive(Serialize)]
    struct Meta {
        #[serde(rename = "denUrl", skip_serializing_if = "Option::is_none")]
        url: Option<&'static str>,
        #[serde(rename = "denPort", skip_serializing_if = "Option::is_none")]
        port: Option<PortLit>,
    }

    #[derive(Clone, Copy, Serialize)]
    #[serde(untagged)]
    enum PortLit {
        Number(u16),
        Wide(i64),
        Float(f64),
        Text(&'static str),
        Bool(bool),
    }

    struct Case {
        id: &'static str,
        name: &'static str,
        host: &'static str,
        caps: &'static [&'static str],
        url: Option<&'static str>,
        port: Option<PortLit>,
        expect: Option<&'static str>,
    }

    fn stamp() -> DateTime<Utc> {
        DateTime::UNIX_EPOCH
    }

    fn node(
        id: &str,
        name: &str,
        host: &str,
        capabilities: &[&str],
        metadata: Option<JsonObject>,
    ) -> MeshNode {
        MeshNode {
            id: id.to_owned(),
            name: name.to_owned(),
            role: None,
            agents: Vec::new(),
            host: host.to_owned(),
            port: 5700,
            providers: Vec::new(),
            models: Vec::new(),
            capabilities: capabilities.iter().copied().map(str::to_owned).collect(),
            status: MeshNodeStatus::Offline,
            last_seen: stamp(),
            registered_at: stamp(),
            version: "0.1.0".to_owned(),
            metadata,
            ssh_user: None,
            install_root: None,
            platform: None,
            extra: Extra::default(),
        }
    }

    fn metadata(
        url: Option<&'static str>,
        port: Option<PortLit>,
    ) -> Result<Option<JsonObject>, serde_json::Error> {
        if url.is_none() && port.is_none() {
            return Ok(None);
        }
        JsonObject::from_serialize(&Meta { url, port }).map(Some)
    }

    fn file(nodes: Vec<MeshNode>) -> MeshFile {
        let mut document = MeshFile::empty(stamp());
        for node in nodes {
            document.nodes.insert(node.id.clone(), node);
        }
        document
    }

    const SAMPLES: [Case; 13] = [
        Case {
            id: "golden-node",
            name: "Golden Node",
            host: "127.0.0.1",
            caps: &[],
            url: Some("http://127.0.0.1:5174"),
            port: None,
            expect: Some("http://127.0.0.1:5174"),
        },
        Case {
            id: "peer-https",
            name: "peer-https",
            host: "peer-https.invalid",
            caps: &[],
            url: Some("https://peer-https.invalid:5174"),
            port: None,
            expect: Some("https://peer-https.invalid:5174"),
        },
        Case {
            id: "peer-cap",
            name: "peer-cap",
            host: "peer-cap.invalid",
            caps: &["den"],
            url: None,
            port: None,
            expect: Some("http://peer-cap.invalid:5174"),
        },
        Case {
            id: "ported",
            name: "ported",
            host: "ported.example",
            caps: &[],
            url: None,
            port: Some(PortLit::Number(6001)),
            expect: Some("http://ported.example:6001"),
        },
        Case {
            id: "text-port",
            name: "text",
            host: "text.example",
            caps: &[],
            url: None,
            port: Some(PortLit::Text(" 6002 ")),
            expect: Some("http://text.example:6002"),
        },
        Case {
            id: "bad-port",
            name: "bad",
            host: "bad.example",
            caps: &["den"],
            url: Some("ftp://bad.example"),
            port: Some(PortLit::Text("nope")),
            expect: Some("http://bad.example:5174"),
        },
        Case {
            id: "zero-port",
            name: "zero",
            host: "zero.example",
            caps: &["den"],
            url: None,
            port: Some(PortLit::Number(0)),
            expect: Some("http://zero.example:5174"),
        },
        Case {
            id: "peer-ftp",
            name: "ftp",
            host: "files.example",
            caps: &[],
            url: Some("ftp://files.example/mesh"),
            port: None,
            expect: None,
        },
        Case {
            id: "agent-only",
            name: "agent",
            host: "agent.invalid",
            caps: &["agent"],
            url: None,
            port: None,
            expect: None,
        },
        Case {
            id: "invalid",
            name: "",
            host: "invalid.example",
            caps: &[],
            url: None,
            port: None,
            expect: None,
        },
        Case {
            id: "wide-port",
            name: "wide",
            host: "wide.example",
            caps: &[],
            url: None,
            port: Some(PortLit::Wide(70_000)),
            expect: None,
        },
        Case {
            id: "float-port",
            name: "float",
            host: "float.example",
            caps: &[],
            url: None,
            port: Some(PortLit::Float(1.5)),
            expect: None,
        },
        Case {
            id: "bool-port",
            name: "bool",
            host: "bool.example",
            caps: &[],
            url: None,
            port: Some(PortLit::Bool(true)),
            expect: None,
        },
    ];

    #[test]
    fn roster_keeps_http_urls_ports_and_the_capability_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = SAMPLES;
        let mut nodes = Vec::new();
        for case in &cases {
            nodes.push(node(
                case.id,
                case.name,
                case.host,
                case.caps,
                metadata(case.url, case.port)?,
            ));
        }
        let overview = project(&file(nodes))?;
        for case in &cases {
            let found = overview
                .nodes
                .iter()
                .find(|node| node.id == case.id)
                .map(|node| node.url.as_str());
            assert_eq!(found, case.expect, "{}", case.id);
        }
        assert!(overview.nodes.iter().all(|node| !node.online));
        assert!(overview.nodes.iter().all(|node| node.sessions.is_none()));
        Ok(())
    }
}
