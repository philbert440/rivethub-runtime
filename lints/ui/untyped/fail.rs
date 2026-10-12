#![allow(dead_code)]

mod serde_json {
    pub type Value = u8;

    pub struct Map<K, V> {
        pub key: K,
        pub value: V,
    }
}

pub struct A {
    pub v: serde_json::Map<String, serde_json::Value>,
}

pub fn main() {}
