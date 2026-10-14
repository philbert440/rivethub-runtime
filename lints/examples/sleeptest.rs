// compile-flags: --test
#[cfg(test)]
mod tests {
    mod tokio {
        pub mod time {
            pub fn sleep(_duration: u64) {}
        }
    }

    #[test]
    fn a_genuine_timing_test_carries_the_expect() {
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                sleep,
                reason = "the fixture holds the stream open past the client's recorded idle window; the wait is the fixture"
            )
        )]
        tokio::time::sleep(400);
    }
}

#[cfg(not(test))]
fn main() {}
