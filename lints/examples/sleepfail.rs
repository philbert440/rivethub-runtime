// compile-flags: --test
#[cfg(test)]
mod tests {
    mod tokio {
        pub mod time {
            pub fn sleep(_duration: u64) {}
            pub fn sleep_until(_duration: u64) {}
        }
    }

    #[test]
    fn a_test_sleeps_instead_of_awaiting_the_event() {
        tokio::time::sleep(20);
    }

    #[test]
    fn a_second_test_sleeps_until_an_instant() {
        tokio::time::sleep_until(20);
    }
}

#[cfg(not(test))]
fn main() {}
