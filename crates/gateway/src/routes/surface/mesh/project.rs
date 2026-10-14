use thiserror::Error;
use url::Url;
use wire::gateway::devices::MeshOverview;
use wire::gateway::devices::MeshOverviewNode;
use wire::mesh::MeshFile;
use wire::mesh::MeshNode;

const DEFAULT_GATEWAY_PORT: u16 = 5174;
const RECORDED_GATEWAY_CAPABILITY: &str = "den";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(super) enum Rule {
    #[error("mesh node id is empty")]
    EmptyId,
    #[error("mesh node name is blank")]
    BlankName,
    #[error("mesh gateway url scheme is not http")]
    NonHttpScheme,
    #[error("mesh gateway host is missing")]
    MissingHost,
    #[error("mesh gateway host is not an origin")]
    UnusableHost,
    #[error("mesh gateway port is zero")]
    ZeroPort,
    #[error("mesh node does not advertise a gateway")]
    NotAGateway,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Withheld {
    pub(super) id: String,
    pub(super) rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub(super) struct Projection {
    pub(super) overview: MeshOverview,
    pub(super) withheld: Vec<Withheld>,
}

pub(super) fn project(file: &MeshFile) -> Projection {
    let projection = classify_all(file);
    for item in &projection.withheld {
        tracing::warn!(
            node_id = %item.id,
            rule = %item.rule,
            "mesh node withheld from the overview"
        );
    }
    projection
}

fn classify_all(file: &MeshFile) -> Projection {
    let mut nodes = Vec::new();
    let mut withheld = Vec::new();
    for node in file.nodes.values() {
        match classify(node) {
            Ok(url) => nodes.push(overview_node(node, url)),
            Err(rule) => withheld.push(Withheld {
                id: node.id.clone(),
                rule,
            }),
        }
    }
    Projection {
        overview: MeshOverview {
            updated_at: file.updated_at,
            nodes,
        },
        withheld,
    }
}

fn overview_node(node: &MeshNode, url: String) -> MeshOverviewNode {
    MeshOverviewNode {
        id: node.id.clone(),
        name: node.name.clone(),
        url,
        online: false,
        sessions: None,
        latest: None,
    }
}

fn classify(node: &MeshNode) -> Result<String, Rule> {
    if node.id.is_empty() {
        return Err(Rule::EmptyId);
    }
    if node.name.trim().is_empty() {
        return Err(Rule::BlankName);
    }
    Ok(advertised(&address(node)?))
}

fn address(node: &MeshNode) -> Result<Url, Rule> {
    if let Some(url) = gateway_url(node) {
        return require_http(url);
    }
    if let Some(port) = gateway_port(node) {
        if port == 0 {
            return Err(Rule::ZeroPort);
        }
        return origin(&node.host, port);
    }
    if advertises_gateway(node) {
        return origin(&node.host, DEFAULT_GATEWAY_PORT);
    }
    Err(Rule::NotAGateway)
}

fn gateway_url(node: &MeshNode) -> Option<Url> {
    node.metadata.as_ref()?.gateway_url.clone()
}

fn gateway_port(node: &MeshNode) -> Option<u16> {
    node.metadata.as_ref()?.gateway_port
}

fn advertises_gateway(node: &MeshNode) -> bool {
    node.capabilities
        .iter()
        .any(|capability| capability == RECORDED_GATEWAY_CAPABILITY)
}

fn require_http(url: Url) -> Result<Url, Rule> {
    match url.scheme() {
        "http" | "https" => Ok(url),
        _ => Err(Rule::NonHttpScheme),
    }
}

fn origin(host: &str, port: u16) -> Result<Url, Rule> {
    if host.is_empty() {
        return Err(Rule::MissingHost);
    }
    match Url::parse(&format!("http://{host}:{port}")) {
        Ok(url) => Ok(url),
        Err(error) => {
            tracing::debug!(host, port, "mesh host is not a gateway origin: {error}");
            Err(Rule::UnusableHost)
        }
    }
}

fn advertised(url: &Url) -> String {
    let text = url.as_str();
    if url.path() == "/" && url.query().is_none() && url.fragment().is_none() {
        match text.strip_suffix('/') {
            Some(origin) => origin.to_owned(),
            None => text.to_owned(),
        }
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::Rule;
    use super::Withheld;
    use super::project;
    use chrono::DateTime;
    use chrono::Utc;
    use url::Url;
    use wire::Extra;
    use wire::mesh::MeshFile;
    use wire::mesh::MeshMetadata;
    use wire::mesh::MeshNode;
    use wire::mesh::MeshNodeStatus;

    struct Case {
        id: &'static str,
        name: &'static str,
        host: &'static str,
        caps: &'static [&'static str],
        url: Option<&'static str>,
        port: Option<u16>,
        included: Option<&'static str>,
        rule: Option<Rule>,
    }

    fn stamp() -> DateTime<Utc> {
        DateTime::UNIX_EPOCH
    }

    fn node(case: &Case) -> Result<MeshNode, url::ParseError> {
        let gateway_url = match case.url {
            Some(text) => Some(Url::parse(text)?),
            None => None,
        };
        let metadata = (gateway_url.is_some() || case.port.is_some()).then(|| MeshMetadata {
            gateway_url,
            gateway_port: case.port,
            extra: Extra::default(),
        });
        Ok(MeshNode {
            id: case.id.to_owned(),
            name: case.name.to_owned(),
            role: None,
            agents: Vec::new(),
            host: case.host.to_owned(),
            port: 5700,
            providers: Vec::new(),
            models: Vec::new(),
            capabilities: case.caps.iter().copied().map(str::to_owned).collect(),
            status: MeshNodeStatus::Offline,
            last_seen: stamp(),
            registered_at: stamp(),
            version: "0.1.0".to_owned(),
            metadata,
            ssh_user: None,
            install_root: None,
            platform: None,
            extra: Extra::default(),
        })
    }

    fn file(cases: &[Case]) -> Result<MeshFile, url::ParseError> {
        let mut document = MeshFile::empty(stamp());
        for case in cases {
            let item = node(case)?;
            document.nodes.insert(case.id.to_owned(), item);
        }
        Ok(document)
    }

    const SAMPLES: &[Case] = &[
        Case {
            id: "golden-node",
            name: "Golden Node",
            host: "127.0.0.1",
            caps: &[],
            url: Some("http://127.0.0.1:5174"),
            port: None,
            included: Some("http://127.0.0.1:5174"),
            rule: None,
        },
        Case {
            id: "peer-https",
            name: "peer-https",
            host: "peer-https.invalid",
            caps: &[],
            url: Some("https://peer-https.invalid:5174"),
            port: None,
            included: Some("https://peer-https.invalid:5174"),
            rule: None,
        },
        Case {
            id: "peer-cap",
            name: "peer-cap",
            host: "peer-cap.invalid",
            caps: &["den"],
            url: None,
            port: None,
            included: Some("http://peer-cap.invalid:5174"),
            rule: None,
        },
        Case {
            id: "ported",
            name: "ported",
            host: "ported.example",
            caps: &[],
            url: None,
            port: Some(6001),
            included: Some("http://ported.example:6001"),
            rule: None,
        },
        Case {
            id: "ftp",
            name: "ftp",
            host: "files.example",
            caps: &["den"],
            url: Some("ftp://files.example/mesh"),
            port: Some(6001),
            included: None,
            rule: Some(Rule::NonHttpScheme),
        },
        Case {
            id: "agent-only",
            name: "agent",
            host: "agent.invalid",
            caps: &["agent"],
            url: None,
            port: None,
            included: None,
            rule: Some(Rule::NotAGateway),
        },
        Case {
            id: "invalid",
            name: "",
            host: "invalid.example",
            caps: &["den"],
            url: Some("http://invalid.example:5174"),
            port: None,
            included: None,
            rule: Some(Rule::BlankName),
        },
        Case {
            id: "spaces",
            name: "   ",
            host: "spaces.example",
            caps: &[],
            url: None,
            port: None,
            included: None,
            rule: Some(Rule::BlankName),
        },
        Case {
            id: "",
            name: "nameless-id",
            host: "empty.example",
            caps: &["den"],
            url: Some("http://empty.example:5174"),
            port: None,
            included: None,
            rule: Some(Rule::EmptyId),
        },
        Case {
            id: "zero",
            name: "zero",
            host: "zero.example",
            caps: &["den"],
            url: None,
            port: Some(0),
            included: None,
            rule: Some(Rule::ZeroPort),
        },
        Case {
            id: "no-host",
            name: "no-host",
            host: "",
            caps: &[],
            url: None,
            port: Some(6001),
            included: None,
            rule: Some(Rule::MissingHost),
        },
        Case {
            id: "bad-host",
            name: "bad-host",
            host: "bad host",
            caps: &[],
            url: None,
            port: Some(6001),
            included: None,
            rule: Some(Rule::UnusableHost),
        },
    ];

    #[test]
    fn gateways_are_projected_and_the_rest_are_withheld_with_a_rule() -> Result<(), url::ParseError>
    {
        let projected = project(&file(SAMPLES)?);
        for case in SAMPLES {
            let found = projected
                .overview
                .nodes
                .iter()
                .find(|node| node.id == case.id)
                .map(|node| node.url.as_str());
            assert_eq!(found, case.included, "{}", case.id);
            let withheld = projected.withheld.iter().find(|item| item.id == case.id);
            assert_eq!(withheld.map(|item| item.rule), case.rule, "{}", case.id);
        }
        assert!(projected.overview.nodes.iter().all(|node| !node.online));
        assert!(
            projected
                .overview
                .nodes
                .iter()
                .all(|node| node.sessions.is_none())
        );
        let ftp = projected
            .withheld
            .iter()
            .find(|item| item.id == "ftp")
            .expect("ftp");
        assert_eq!(
            ftp,
            &Withheld {
                id: "ftp".to_owned(),
                rule: Rule::NonHttpScheme,
            }
        );
        Ok(())
    }

    #[test]
    fn a_cutover_string_port_projects_to_that_origin() {
        let text = r#"{"version":1,"updatedAt":0,"nodes":{"ported":{"id":"ported","name":"ported","agents":[],"host":"ported.example","port":5700,"providers":[],"models":[],"capabilities":[],"status":"offline","lastSeen":0,"registeredAt":0,"version":"0.1.0","metadata":{"denPort":"6002"}}}}"#;
        let file: MeshFile = serde_json::from_str(text).expect("mesh file");
        let projected = project(&file);
        assert_eq!(projected.withheld, Vec::<Withheld>::new());
        assert_eq!(
            projected
                .overview
                .nodes
                .first()
                .map(|node| node.url.as_str()),
            Some("http://ported.example:6002")
        );
    }
}
