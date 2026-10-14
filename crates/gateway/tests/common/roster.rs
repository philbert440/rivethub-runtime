use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use wire::Extra;
use wire::JsonObject;
use wire::mesh::MeshFile;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;

pub(crate) struct Member {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) host: &'static str,
    pub(crate) url: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) text_port: Option<&'static str>,
    pub(crate) capabilities: &'static [&'static str],
}

pub(crate) fn recorded(port: u16) -> Result<Vec<u8>, String> {
    let local = format!("http://127.0.0.1:{port}");
    document(
        &[
            Member {
                id: "golden-node",
                name: "Golden Node",
                host: "127.0.0.1",
                url: Some(local),
                port: None,
                text_port: None,
                capabilities: &[],
            },
            Member {
                id: "peer-refused",
                name: "Peer Refused",
                host: "127.0.0.1",
                url: Some("http://127.0.0.1:1".to_owned()),
                port: None,
                text_port: None,
                capabilities: &[],
            },
            Member {
                id: "peer-https",
                name: "peer-https",
                host: "peer-https.invalid",
                url: Some("https://peer-https.invalid:5174".to_owned()),
                port: None,
                text_port: None,
                capabilities: &[],
            },
            Member {
                id: "peer-cap",
                name: "peer-cap",
                host: "peer-cap.invalid",
                url: None,
                port: None,
                text_port: None,
                capabilities: &["den"],
            },
            Member {
                id: "peer-ftp",
                name: "ftp",
                host: "files.example",
                url: Some("ftp://files.example/mesh".to_owned()),
                port: None,
                text_port: None,
                capabilities: &[],
            },
            Member {
                id: "agent-only",
                name: "agent",
                host: "agent.invalid",
                url: None,
                port: None,
                text_port: None,
                capabilities: &["agent"],
            },
            Member {
                id: "invalid",
                name: "",
                host: "invalid.example",
                url: None,
                port: None,
                text_port: None,
                capabilities: &[],
            },
        ],
        1_760_000_000_000,
    )
}

pub(crate) fn document(members: &[Member], updated_millis: i64) -> Result<Vec<u8>, String> {
    let updated_at = DateTime::<Utc>::from_timestamp_millis(updated_millis)
        .ok_or_else(|| format!("timestamp {updated_millis}"))?;
    let mut file = MeshFile::empty(updated_at);
    for member in members {
        let node = mesh_node(member)?;
        file.nodes.insert(node.id.clone(), node);
    }
    serde_json::to_vec(&file).map_err(|error| error.to_string())
}

pub(crate) fn flat() -> Result<Vec<u8>, String> {
    let node = mesh_node(&Member {
        id: "x",
        name: "n",
        host: "127.0.0.1",
        url: None,
        port: None,
        text_port: None,
        capabilities: &[],
    })?;
    serde_json::to_vec(&Flat { nodes: vec![node] }).map_err(|error| error.to_string())
}

fn mesh_node(member: &Member) -> Result<MeshNode, String> {
    Ok(MeshNode {
        id: member.id.to_owned(),
        name: member.name.to_owned(),
        role: None,
        agents: Vec::new(),
        host: member.host.to_owned(),
        port: 5700,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: member
            .capabilities
            .iter()
            .copied()
            .map(str::to_owned)
            .collect(),
        status: MeshNodeStatus::Offline,
        last_seen: DateTime::UNIX_EPOCH,
        registered_at: DateTime::UNIX_EPOCH,
        version: "0.1.0".to_owned(),
        metadata: metadata(member)?,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: Extra::default(),
    })
}

fn metadata(member: &Member) -> Result<Option<JsonObject>, String> {
    if member.url.is_none() && member.port.is_none() && member.text_port.is_none() {
        return Ok(None);
    }
    let port = match (member.port, member.text_port) {
        (Some(port), _) => Some(PortSer::Number(port)),
        (None, Some(text)) => Some(PortSer::Text(text)),
        (None, None) => None,
    };
    JsonObject::from_serialize(&Meta {
        url: member.url.clone(),
        port,
    })
    .map(Some)
    .map_err(|error| error.to_string())
}

#[derive(Serialize)]
struct Flat {
    nodes: Vec<MeshNode>,
}

#[derive(Serialize)]
struct Meta {
    #[serde(rename = "denUrl", skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(rename = "denPort", skip_serializing_if = "Option::is_none")]
    port: Option<PortSer>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum PortSer {
    Number(u16),
    Text(&'static str),
}
