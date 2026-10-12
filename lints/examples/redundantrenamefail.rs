use serde::Serialize;

#[derive(Serialize)]
struct A {
    #[serde(rename = "max_tokens")]
    max_tokens: u32,
}

fn main() {
    let _ = A { max_tokens: 0 };
}
