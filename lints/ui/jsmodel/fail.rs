#![allow(dead_code)]

pub fn is_truthy(value: &str) -> bool {
    !value.is_empty()
}

pub struct JsValue;

pub enum NumberOrText {
    Text(String),
}

pub fn build() -> bool {
    "[object Object]".is_empty()
}

pub fn main() {}
