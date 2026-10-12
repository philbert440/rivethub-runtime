#![allow(dead_code)]

pub fn a(s: &String) -> u8 {
    match s.as_str() {
        _ => 1,
    }
}

pub fn main() {}
