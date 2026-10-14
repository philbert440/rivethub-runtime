use std::fmt::Write;
use std::path::Path;
use std::str::FromStr;

use super::FilesError;

const OCTET_STREAM: &str = "application/octet-stream";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineType {
    Png,
    Jpg,
    Jpeg,
    Gif,
    Webp,
    Avif,
    Ico,
    Txt,
    Md,
    Json,
    Csv,
    Pdf,
    Log,
    Xml,
    Mp3,
    Wav,
    Mp4,
    Webm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotInline;

impl InlineType {
    const fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpg | Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Ico => "image/x-icon",
            Self::Txt | Self::Md | Self::Log | Self::Xml => "text/plain; charset=utf-8",
            Self::Json => "application/json",
            Self::Csv => "text/csv; charset=utf-8",
            Self::Pdf => "application/pdf",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Mp4 => "video/mp4",
            Self::Webm => "video/webm",
        }
    }
}

impl FromStr for InlineType {
    type Err = NotInline;

    fn from_str(ext: &str) -> Result<Self, Self::Err> {
        match ext {
            "png" => Ok(Self::Png),
            "jpg" => Ok(Self::Jpg),
            "jpeg" => Ok(Self::Jpeg),
            "gif" => Ok(Self::Gif),
            "webp" => Ok(Self::Webp),
            "avif" => Ok(Self::Avif),
            "ico" => Ok(Self::Ico),
            "txt" => Ok(Self::Txt),
            "md" => Ok(Self::Md),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            "pdf" => Ok(Self::Pdf),
            "log" => Ok(Self::Log),
            "xml" => Ok(Self::Xml),
            "mp3" => Ok(Self::Mp3),
            "wav" => Ok(Self::Wav),
            "mp4" => Ok(Self::Mp4),
            "webm" => Ok(Self::Webm),
            _ => Err(NotInline),
        }
    }
}

pub(super) struct Classified {
    pub content_type: &'static str,
    pub inline: bool,
}

pub(super) fn classify(name: &str) -> Classified {
    let kind = match extension(name) {
        Some(ext) => InlineType::from_str(&ext),
        None => Err(NotInline),
    };
    match kind {
        Ok(kind) => Classified {
            content_type: kind.content_type(),
            inline: true,
        },
        Err(NotInline) => Classified {
            content_type: OCTET_STREAM,
            inline: false,
        },
    }
}

pub(super) fn content_disposition(inline: bool, name: &str) -> Result<String, FilesError> {
    let kind = if inline { "inline" } else { "attachment" };
    let value = if quoted_filename(name) {
        format!("{kind}; filename=\"{name}\"")
    } else {
        format!(
            "{kind}; filename*=UTF-8''{}",
            percent_encode(name).map_err(|error| FilesError::DownloadFailed(error.to_string()))?
        )
    };
    if axum::http::HeaderValue::from_str(&value).is_err() {
        return Err(FilesError::DownloadFailed(value));
    }
    Ok(value)
}

fn extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

fn quoted_filename(name: &str) -> bool {
    !name.is_empty()
        && name.is_ascii()
        && name
            .chars()
            .all(|ch| !ch.is_control() && ch != '"' && ch != '\\')
}

fn percent_encode(name: &str) -> Result<String, std::fmt::Error> {
    let mut encoded = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            write!(encoded, "%{byte:02X}")?;
        }
    }
    Ok(encoded)
}
