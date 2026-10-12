#![allow(dead_code)]

pub struct A(u8);

impl PartialEq for A {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

pub fn main() {}
