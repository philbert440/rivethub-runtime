use serde::Serialize;

#[derive(Serialize)]
struct A {
    hidden: u8,
}

fn main() {
    let _ = A { hidden: 0 };
}
