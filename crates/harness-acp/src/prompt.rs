use agent_client_protocol::schema::v1 as acp;
use base64::Engine as _;
use kernel::harness::TurnAttachment;
use kernel::harness::TurnInput;

use crate::error::AcpError;

#[derive(Debug, Clone, Copy, strum::Display, strum::EnumString)]
enum InlineImage {
    #[strum(serialize = "image/png")]
    Png,
    #[strum(serialize = "image/jpeg")]
    Jpeg,
    #[strum(serialize = "image/gif")]
    Gif,
    #[strum(serialize = "image/webp")]
    Webp,
}

pub(crate) async fn content(
    input: TurnInput,
    images: bool,
) -> Result<Vec<acp::ContentBlock>, AcpError> {
    let (text, attachments) = match input {
        TurnInput::Text(text) => (text, Vec::new()),
        TurnInput::Attached { text, attachments } => (text, attachments),
    };
    let mut blocks = Vec::with_capacity(attachments.len() + 1);
    if !text.is_empty() {
        blocks.push(acp::ContentBlock::from(text));
    }
    for attachment in attachments {
        blocks.push(block(attachment, images).await?);
    }
    Ok(blocks)
}

async fn block(attachment: TurnAttachment, images: bool) -> Result<acp::ContentBlock, AcpError> {
    let uri = format!("file://{}", attachment.path.display());
    if images && let Ok(inline) = essence(&attachment.media_type).parse::<InlineImage>() {
        let bytes =
            tokio::fs::read(&attachment.path)
                .await
                .map_err(|source| AcpError::Attachment {
                    path: attachment.path.clone(),
                    source,
                })?;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        return Ok(acp::ContentBlock::Image(
            acp::ImageContent::new(data, inline.to_string()).uri(uri),
        ));
    }
    Ok(acp::ContentBlock::ResourceLink(
        acp::ResourceLink::new(attachment.name, uri).mime_type(attachment.media_type),
    ))
}

fn essence(media_type: &str) -> String {
    let base = match media_type.split_once(';') {
        Some((base, _)) => base,
        None => media_type,
    };
    base.trim().to_ascii_lowercase()
}
