use super::{events::text_blocks_in, StoredEvent, TimelineChunk};

/// Canonical Codex agent messages start with a synthetic routing block;
/// subsequent blocks contain the available message body, including Markdown.
pub(super) fn project(event: &StoredEvent) -> Option<TimelineChunk> {
    if event.agent != "codex" || event.subtype.as_deref() != Some("agent_message") {
        return None;
    }
    let blocks = text_blocks_in(event);
    let route = blocks.first()?.strip_prefix("Agent message: ")?;
    let (author, recipient) = route.split_once(" → ")?;
    let text = blocks[1..].join("\n\n");
    let envelope = unwrap_envelope(&text);
    let message_type = envelope.map(|(kind, _)| kind.to_owned());
    let body = envelope.map_or(text.as_str(), |(_, body)| body);
    Some(TimelineChunk::AgentMessage {
        author: author.to_owned(),
        recipient: recipient.to_owned(),
        message_type,
        text: body.to_owned(),
    })
}

/// Strip only a complete harness envelope, leaving ordinary message text alone.
fn unwrap_envelope(text: &str) -> Option<(&str, &str)> {
    let (message_type, rest) = text.strip_prefix("Message Type: ")?.split_once('\n')?;
    let (_, rest) = rest.strip_prefix("Task name: ")?.split_once('\n')?;
    let (_, rest) = rest.strip_prefix("Sender: ")?.split_once('\n')?;
    let body = rest.strip_prefix("Payload:")?;
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .or_else(|| body.strip_prefix(' '))
        .unwrap_or(body);
    Some((message_type.trim(), body))
}
