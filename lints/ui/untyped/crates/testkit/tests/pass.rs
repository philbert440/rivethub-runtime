// compile-flags: --edition=2021
#![allow(dead_code)]
#![feature(decl_macro)]

mod serde_json {
    pub(crate) macro json($($tt:tt)*) {
        0
    }

    pub type Value = u8;
}

pub fn fixture() -> Option<serde_json::Value> {
    let args = serde_json::json!({"text": "ping"});
    Some(args)
}

pub fn main() {}
