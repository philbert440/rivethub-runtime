#![allow(dead_code)]

mod tokio {
    pub mod time {
        pub fn sleep(_duration: u64) {}
        pub fn sleep_until(_duration: u64) {}
    }
}

pub fn production_timers_legitimately_sleep() {
    tokio::time::sleep(20);
    let _ = tokio::time::sleep_until(20);
}

pub fn main() {}
