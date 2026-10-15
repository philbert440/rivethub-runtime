use super::ImportError;

pub(super) fn invalid(line: usize, detail: impl Into<String>) -> ImportError {
    ImportError::Format {
        line,
        detail: detail.into(),
    }
}

pub(super) fn envelope(text: &str, line: usize) -> Result<(), ImportError> {
    if text.is_empty() || text.starts_with("--") {
        return Ok(());
    }
    if let Some(token) = text
        .strip_prefix("\\restrict ")
        .or_else(|| text.strip_prefix("\\unrestrict "))
        && !token.is_empty()
        && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Ok(());
    }
    if text == "SELECT pg_catalog.set_config('search_path', '', false);" {
        return Ok(());
    }
    if let Some(setting) = text
        .strip_prefix("SET ")
        .and_then(|text| text.strip_suffix(';'))
        && let Some((key, value)) = setting.split_once(" = ")
    {
        let supported = match key {
            "statement_timeout"
            | "lock_timeout"
            | "idle_in_transaction_session_timeout"
            | "transaction_timeout" => value == "0",
            "client_encoding" => value == "'UTF8'",
            "standard_conforming_strings" => value == "on",
            "check_function_bodies" | "row_security" => value == "false" || value == "off",
            "client_min_messages" => value == "warning",
            "xmloption" => value == "content",
            _ => false,
        };
        if supported {
            return Ok(());
        }
    }
    Err(invalid(
        line,
        "unsupported statement; use pg_dump --data-only --format=plain with the supported memory tables",
    ))
}

pub(super) fn field(text: &str, line: usize) -> Result<Option<String>, ImportError> {
    if text == "\\N" {
        return Ok(None);
    }
    let mut input = text.bytes().peekable();
    let mut output = Vec::with_capacity(text.len());
    while let Some(byte) = input.next() {
        if byte == 0 {
            return Err(invalid(line, "NUL is not Postgres text"));
        }
        if byte != b'\\' {
            output.push(byte);
            continue;
        }
        let escaped = input
            .next()
            .ok_or_else(|| invalid(line, "unfinished COPY escape"))?;
        let decoded = match escaped {
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            b'\\' => b'\\',
            b'0'..=b'7' => {
                let mut value = u16::from(escaped - b'0');
                for _ in 0..2 {
                    if let Some(next @ b'0'..=b'7') = input.peek().copied() {
                        input.next();
                        value = value * 8 + u16::from(next - b'0');
                    } else {
                        break;
                    }
                }
                u8::try_from(value)
                    .map_err(|_| invalid(line, "COPY octal escape exceeds a byte"))?
            }
            b'x' => {
                let mut digits = String::new();
                for _ in 0..2 {
                    if let Some(next) = input.peek().copied().filter(u8::is_ascii_hexdigit) {
                        input.next();
                        digits.push(char::from(next));
                    } else {
                        break;
                    }
                }
                u8::from_str_radix(&digits, 16)
                    .map_err(|_| invalid(line, "invalid COPY hexadecimal escape"))?
            }
            _ => return Err(invalid(line, "unsupported COPY escape")),
        };
        if decoded == 0 {
            return Err(invalid(line, "NUL is not Postgres text"));
        }
        output.push(decoded);
    }
    String::from_utf8(output)
        .map(Some)
        .map_err(|_| invalid(line, "COPY field is not UTF-8"))
}
