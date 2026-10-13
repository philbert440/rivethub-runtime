// compile-flags: --edition=2021
#![allow(dead_code)]
#![feature(decl_macro)]

mod serde_json {
    pub(crate) macro json($($tt:tt)*) {
        0
    }

    pub type Value = u8;
}

pub fn build() -> Option<serde_json::Value> {
    None
}

pub fn frame() -> i32 {
    serde_json::json!({"kind": "ping"})
}

pub fn main() {}
