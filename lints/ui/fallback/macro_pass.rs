#[expect(typemacro, reason = "the fixture exercises expectations after expansion")]
macro_rules! documented {
    ($value:expr) => {{
        #[expect(fallback, reason = "CONTRACT B10: this setting is optional")]
        let number = $value.ok();
        number
    }};
}

fn main() {
    let _ = documented!(Ok::<u32, ()>(7));
}
