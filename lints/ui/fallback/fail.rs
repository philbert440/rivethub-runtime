#![allow(dead_code)]

fn best_effort_name_is_not_an_escape(value: Result<u32, ()>) -> Option<u32> {
    value.ok()
}

fn result_defaults(value: Result<u32, ()>) {
    let _ = value.unwrap_or(42);
    let _ = value.unwrap_or_else(|_| 42);
    let _ = value.unwrap_or_default();
    let _ = value.map_or(42, |n| n + 1);
    let _ = value.map_or_else(|_| 42, |n| n + 1);
    let _ = Result::ok(value);
}

fn absent_literals(value: Option<&str>, number: Option<i32>) {
    let _ = value.unwrap_or("");
    let _ = value.unwrap_or("unknown");
    let _ = value.unwrap_or_else(|| "default");
    let _ = number.unwrap_or(0);
    let _ = number.unwrap_or(-1);
}

fn error_values(value: Result<u32, ()>) -> u32 {
    match value {
        Ok(value) => value,
        Err(_) => 7,
    }
}

fn error_unit(value: Result<u32, ()>) {
    match value {
        Ok(_) => (),
        Err(_) => {},
    }
}

fn let_else_value(value: Result<u32, ()>) -> u32 {
    let Ok(value) = value else { return 7 };
    value
}

fn let_else_skip(values: Vec<Result<u32, ()>>) {
    for value in values {
        let Ok(_value) = value else { continue };
    }
}

fn adjusted_receiver(value: Box<Result<u32, ()>>) {
    let _ = value.unwrap_or(42);
}

fn alias(value: Result<u32, ()>) -> u32 {
    use std::result::Result::{Err as Failure, Ok as Success};
    match value {
        Success(value) => value,
        Failure(_) => 42,
    }
}

fn main() {}
