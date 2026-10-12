use serde::Serialize;

#[derive(Serialize)]
struct A {
    #[serde(rename = "agentId")]
    agent_id: u32,
}

fn main() {
    let _ = A { agent_id: 0 };
}
