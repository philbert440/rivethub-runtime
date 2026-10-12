#![allow(dead_code)]
#![allow(unused_macros)]

macro_rules! closed {
    ($n:ident) => {
        pub enum $n {}
    };
}

pub fn main() {}
