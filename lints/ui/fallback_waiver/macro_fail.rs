#[expect(typemacro, reason = "the fixture exercises expectations after expansion")]
macro_rules! silent {
    ($value:expr) => {{
        #[expect(fallback, reason = "a default")]
        let number = $value.ok();
        number
    }};
}

#[expect(typemacro, reason = "the fixture exercises item expectations after expansion")]
macro_rules! broad {
    () => {
        #[expect(fallback, reason = "CONTRACT B10: an item is still too broad")]
        fn converted(value: Result<u32, ()>) -> Option<u32> {
            value.ok()
        }
    };
}

broad!();

fn main() {
    let _ = converted(Ok(7));
    let _ = silent!(Ok::<u32, ()>(7));
}
