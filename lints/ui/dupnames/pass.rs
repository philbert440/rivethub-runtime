#![allow(dead_code)]

pub enum H {
    A,
    B,
}

pub fn n(h: H) -> &'static str {
    match h {
        H::A => "a",
        H::B => "b",
    }
}

pub fn main() {}
