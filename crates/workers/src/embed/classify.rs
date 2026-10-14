use kernel::memory::Unembeddable;

const PNG_MARKER: &str = "iVBORw0KGgo";
const JPEG_MARKER: &str = "/9j/";
const DATA_URL_MARK: &str = ";base64,";
const BLOB_RUN: usize = 1_500;
const BLOB_WINDOW: usize = 4_000;
const DENSITY_PERCENT: usize = 95;
const PERCENT_SCALE: usize = 100;
const PNG_TAIL: usize = 200;
const JPEG_TAIL: usize = 500;
const DATA_URL_TAIL: usize = 200;

#[must_use]
pub fn classify_unembeddable(text: &str) -> Option<Unembeddable> {
    let trimmed = text.trim();
    if trimmed.starts_with("[media attached:") {
        return Some(Unembeddable::MediaMarker);
    }
    if trimmed.starts_with("MEDIA:") {
        return Some(Unembeddable::MediaPrefix);
    }
    if data_url(trimmed) {
        return Some(Unembeddable::Base64DataUrl);
    }
    if marker_tail(trimmed, PNG_MARKER, PNG_TAIL) {
        return Some(Unembeddable::Base64Png);
    }
    if marker_tail(trimmed, JPEG_MARKER, JPEG_TAIL) {
        return Some(Unembeddable::Base64Jpeg);
    }
    if base64_blob(text) {
        return Some(Unembeddable::Base64Blob);
    }
    None
}

fn data_url(text: &str) -> bool {
    let Some(mark) = text.find(DATA_URL_MARK) else {
        return false;
    };
    let Some(head) = text.get(..mark) else {
        return false;
    };
    if !head.contains("data:") {
        return false;
    }
    let Some(tail) = text.get(mark + DATA_URL_MARK.len()..) else {
        return false;
    };
    base64_run(tail) >= DATA_URL_TAIL
}

fn marker_tail(text: &str, marker: &str, minimum: usize) -> bool {
    let Some(at) = text.find(marker) else {
        return false;
    };
    let Some(tail) = text.get(at + marker.len()..) else {
        return false;
    };
    base64_run(tail) >= minimum
}

fn base64_blob(text: &str) -> bool {
    let window: String = text.chars().take(BLOB_WINDOW).collect();
    if window.is_empty() {
        return false;
    }
    let base64_chars = window.chars().filter(|ch| is_base64(*ch)).count();
    let window_chars = window.chars().count();
    let dense = base64_chars.saturating_mul(PERCENT_SCALE) > window_chars * DENSITY_PERCENT;
    dense && longest_base64_run(&window) >= BLOB_RUN
}

fn base64_run(text: &str) -> usize {
    text.chars().take_while(|ch| is_base64(*ch)).count()
}

fn longest_base64_run(text: &str) -> usize {
    let mut best = 0;
    let mut length = 0;
    for ch in text.chars() {
        if is_base64(ch) {
            length += 1;
            best = best.max(length);
        } else {
            length = 0;
        }
    }
    best
}

fn is_base64(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '+' || ch == '/' || ch == '='
}
