pub(super) enum PercentDecode {
    BadEscape,
}

pub(super) fn strict_percent_decode(input: &str) -> Result<String, PercentDecode> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().ok_or(PercentDecode::BadEscape)?;
        if byte == b'%' {
            let hi_at = index.checked_add(1).ok_or(PercentDecode::BadEscape)?;
            let lo_at = index.checked_add(2).ok_or(PercentDecode::BadEscape)?;
            let hi = hex_value(bytes.get(hi_at).copied().ok_or(PercentDecode::BadEscape)?)?;
            let lo = hex_value(bytes.get(lo_at).copied().ok_or(PercentDecode::BadEscape)?)?;
            out.push((hi << 4) | lo);
            index = lo_at.saturating_add(1);
        } else {
            out.push(byte);
            index = index.saturating_add(1);
        }
    }
    String::from_utf8(out).map_err(|_| PercentDecode::BadEscape)
}

pub(super) fn phone_name_ok(name: &str) -> bool {
    let count = name.chars().count();
    (1..=64).contains(&count)
        && name
            .chars()
            .all(|ch| matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-'))
}

pub(super) fn phone_host_ok(host: &str) -> bool {
    let count = host.chars().count();
    (1..=253).contains(&count)
        && host.chars().all(|ch| {
            matches!(
                ch,
                'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | ':' | '[' | ']' | '-'
            )
        })
}

fn hex_value(byte: u8) -> Result<u8, PercentDecode> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(PercentDecode::BadEscape),
    }
}
