// compile-flags: --edition=2021
#![allow(dead_code)]
#![feature(decl_macro)]

mod serde_json {
    pub(crate) macro json($($tt:tt)*) {
        0
    }
}

pub fn fixture() -> i32 {
    serde_json::json!({"kind": "ping"})
}

pub fn main() {}
