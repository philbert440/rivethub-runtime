#![allow(dead_code)]

fn meaningful(value: Option<u32>) -> u32 {
    value.unwrap_or(42)
}

fn absent_collection(value: Option<Vec<u32>>) -> Vec<u32> {
    value.unwrap_or_default()
}

fn propagate(value: Result<u32, ()>) -> Result<u32, ()> {
    let Ok(value) = value else { return Err(()) };
    Ok(value)
}

fn explicit_error(value: Result<u32, ()>) -> Result<u32, ()> {
    match value {
        Ok(value) => Ok(value),
        Err(_) => Err(()),
    }
}

fn fatal(value: Result<u32, ()>) -> u32 {
    value.unwrap_or_else(|_| panic!("fatal"))
}

fn documented(value: Result<u32, ()>) -> u32 {
    #[expect(fallback, reason = "CONTRACT B10: the configured default is 42")]
    let number = value.unwrap_or(42);
    number
}

fn documented_pattern(value: Result<u32, ()>) -> u32 {
    #[expect(fallback, reason = "PATTERNS FALLBACK: the final report uses a count of zero")]
    let number = value.unwrap_or(0);
    number
}

struct Custom;
impl Custom {
    fn unwrap_or(self, n: u32) -> u32 { n }
    fn ok(self) -> u32 { 7 }
}

fn unrelated() {
    let _ = Custom.unwrap_or(0);
    let _ = Custom.ok();
}

fn logged_propagation(value: Result<u32, ()>) -> Result<u32, ()> {
    let Ok(number) = value else {
        eprintln!("invalid input");
        return Err(());
    };
    Ok(number)
}

fn borrowed_option(value: &Option<u32>) -> u32 {
    value.unwrap_or(42)
}

fn main() {}
