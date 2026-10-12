use serde::Serialize;

#[derive(Serialize)]
struct A {
    pub shown: u8,
}

#[derive(Serialize)]
struct Id(String);

fn main() {
    let _ = A { shown: 0 };
    let _ = Id("00000000-0000-4000-8000-000000000000".to_owned());
}
