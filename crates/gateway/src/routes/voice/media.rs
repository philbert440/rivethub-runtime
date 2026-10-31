use axum::http::HeaderValue;

const WAV: &str = "audio/wav";

pub(super) fn forwarded_media(header: Option<&HeaderValue>) -> String {
    let Some(value) = header else {
        return WAV.to_owned();
    };
    if let Ok(text) = value.to_str() {
        named_media(text)
    } else {
        WAV.to_owned()
    }
}

fn named_media(text: &str) -> String {
    let Some(media) = text.split(';').next() else {
        return WAV.to_owned();
    };
    let media = media.trim();
    if media.is_empty() {
        return WAV.to_owned();
    }
    let lower = media.to_ascii_lowercase();
    if allowed(&lower) {
        lower
    } else {
        WAV.to_owned()
    }
}

fn allowed(media: &str) -> bool {
    matches!(
        media,
        "audio/wav"
            | "audio/wave"
            | "audio/x-wav"
            | "audio/mpeg"
            | "audio/mp3"
            | "audio/mp4"
            | "audio/m4a"
            | "audio/x-m4a"
            | "audio/aac"
            | "audio/webm"
            | "audio/ogg"
            | "audio/flac"
            | "application/octet-stream"
            | "video/webm"
    )
}
