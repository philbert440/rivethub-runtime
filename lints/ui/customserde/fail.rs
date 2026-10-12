#![allow(dead_code)]

pub mod serde {
    pub trait Serialize {}
}

pub struct V;

impl serde::Serialize for V {}

pub fn main() {}
