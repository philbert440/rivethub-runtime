// compile-flags: --test
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_test_macro_builds_no_flagged_runtime() {}
}

#[cfg(not(test))]
fn main() {}
