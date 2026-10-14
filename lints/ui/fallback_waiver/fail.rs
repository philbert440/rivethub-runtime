#![allow(dead_code)]

#[expect(fallback, reason = "CONTRACT B10: a default must be scoped to a statement")]
fn too_broad(value: Result<u32, ()>) -> u32 {
    value.unwrap_or(42)
}

fn no_citation(value: Result<u32, ()>) -> u32 {
    #[expect(fallback, reason = "documented default")]
    let number = value.unwrap_or(42);
    number
}

fn no_reason(value: Result<u32, ()>) -> u32 {
    #[expect(fallback)]
    let number = value.unwrap_or(42);
    number
}

fn fake_citation(value: Result<u32, ()>) -> u32 {
    #[expect(fallback, reason = "CONTRACT B999 and PATTERNS NONEXISTENT")]
    let number = value.unwrap_or(42);
    number
}

fn main() {}
