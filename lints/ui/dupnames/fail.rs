#![allow(dead_code)]

pub enum H {
    A,
    B,
    C,
}

pub fn n(h: H) -> &'static str {
    match h {
        H::A => "a",
        H::B => "b",
        H::C => "c",
    }
}

pub fn main() {}
