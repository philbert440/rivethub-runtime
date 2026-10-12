#![allow(dead_code)]

pub enum Kind {
    A,
}

pub fn a(k: Kind) -> u8 {
    match k {
        Kind::A => 1,
    }
}

pub fn main() {}
