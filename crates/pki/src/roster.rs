use std::collections::BTreeSet;
use std::net::IpAddr;

use kernel::ca::Principal;

use crate::Error;

#[derive(Debug)]
pub struct Roster {
    nodes: Vec<Node>,
}

#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) address: IpAddr,
    pub(crate) agent: Option<String>,
}

struct Fields<'a> {
    name: &'a str,
    address: &'a str,
    agent: &'a str,
}

pub fn parse_roster(text: &str) -> Result<Roster, Error> {
    let mut nodes = Vec::new();
    let mut seen_nodes = BTreeSet::new();
    let mut seen_agents = BTreeSet::new();
    let mut seen_addresses = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let number = index
            .checked_add(1)
            .ok_or(Error::RosterLine { line: index })?;
        let fields = fields(line, number)?;
        let name = label(fields.name, number)?;
        let address = address(fields.address, number)?;
        let agent = agent(fields.agent, number)?;
        if !seen_nodes.insert(name.clone()) {
            return Err(Error::RosterNode { line: number, name });
        }
        if !seen_addresses.insert(address) {
            return Err(Error::RosterAddress {
                line: number,
                address,
            });
        }
        if let Some(agent_name) = &agent
            && !seen_agents.insert(agent_name.clone())
        {
            return Err(Error::RosterAgent {
                line: number,
                name: agent_name.clone(),
            });
        }
        nodes.push(Node {
            name,
            address,
            agent,
        });
    }
    Ok(Roster { nodes })
}

impl Roster {
    pub fn agent_for_node(&self, name: &str) -> Result<Option<&str>, Error> {
        Ok(self.node(name)?.agent.as_deref())
    }

    pub(crate) fn node(&self, name: &str) -> Result<&Node, Error> {
        self.nodes
            .iter()
            .find(|node| node.name == name)
            .ok_or_else(|| Error::UnknownNode(name.to_owned()))
    }

    pub(crate) fn agent<'a>(&'a self, name: &str) -> Result<(&'a Node, &'a str), Error> {
        for node in &self.nodes {
            if let Some(agent) = &node.agent
                && agent == name
            {
                return Ok((node, agent.as_str()));
            }
        }
        Err(Error::UnknownAgent(name.to_owned()))
    }
}

fn fields(line: &str, number: usize) -> Result<Fields<'_>, Error> {
    let mut parts = line.split(':');
    let name = parts.next().ok_or(Error::RosterLine { line: number })?;
    let address = parts.next().ok_or(Error::RosterLine { line: number })?;
    let agent = parts.next().ok_or(Error::RosterLine { line: number })?;
    if parts.next().is_some() || name.is_empty() || address.is_empty() {
        Err(Error::RosterLine { line: number })
    } else {
        Ok(Fields {
            name,
            address,
            agent,
        })
    }
}

fn label(value: &str, number: usize) -> Result<String, Error> {
    Principal::Node {
        node: value.to_owned(),
    }
    .spiffe_uri()
    .map(|_| value.to_owned())
    .map_err(|_| Error::RosterLine { line: number })
}

fn address(value: &str, number: usize) -> Result<IpAddr, Error> {
    let address = value
        .parse::<IpAddr>()
        .map_err(|_| Error::RosterLine { line: number })?;
    if address.is_ipv4() {
        Ok(address)
    } else {
        Err(Error::RosterLine { line: number })
    }
}

fn agent(value: &str, number: usize) -> Result<Option<String>, Error> {
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(label(value, number)?))
    }
}
