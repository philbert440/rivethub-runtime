use kernel::memory::TagJobContext;
use kernel::memory::TagOnEntity;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagState;

pub const TAG_SYSTEM_PROMPT: &str = "You tag one conversation summary so it can be found later by project and topic. Reply with a JSON array only. An empty array [] is a good answer; most conversations need zero or one new tag. Suggest at most 3 tags. Reuse an existing vocabulary entry whenever one fits, including when the conversation names one of its aliases. Propose a new value only when the conversation is substantially about something no entry covers and that will come up again in other conversations. Use key \"project\" only for a named product, repository or long-running effort. Never use a folder or path name, temp dir, worktree or lane name, branch, task-brief file, ticket, PR number, version, review round, or an agent, model or harness name as a value. Use key \"topic\" for a durable subject area such as networking, storage or gpu-inference, not for a one-off task, a status or an activity like code review. Values are short lowercase hyphenated nouns. Each item has key, value, an optional confidence from 0 to 1, and an optional reason. Set action to \"remove\" only for a tag already on the conversation, and only when the summary shows that tag is wrong. Omit action to add a tag.";

pub(super) const MAX_TAG_TOKENS: u32 = 400;

pub fn tag_user_prompt(context: &TagJobContext) -> String {
    let title = conversation_title(context);
    let vocabulary = vocabulary_lines(context).join("\n");
    let current = current_block(context);
    format!(
        "Conversation title: {title}\n\nAgent: {agent}\n\nExisting vocabulary (reuse when it fits):\n{vocabulary}\n\n{current}\n\nSummary:\n{summary}\n\nTags (JSON array only):",
        agent = context.agent,
        summary = context.summary,
    )
}

fn conversation_title(context: &TagJobContext) -> String {
    match context.title.as_deref() {
        Some(title) if !title.is_empty() => title.to_owned(),
        Some(_) | None => format!("Session {}", context.session_key),
    }
}

fn vocabulary_lines(context: &TagJobContext) -> Vec<String> {
    let mut lines = Vec::new();
    let mut seen = Vec::new();
    for entry in context
        .vocabulary
        .iter()
        .filter(|entry| entry.state != TagState::Rejected)
    {
        lines.push(with_aliases(
            bullet(&entry.key, &entry.value, &entry.display),
            &entry.aliases,
        ));
        seen.push(pair(&entry.key, &entry.value));
    }
    for tag in accepted_conversation(context) {
        let key = pair(&tag.key, &tag.value);
        if seen.iter().any(|seen| seen == &key) {
            continue;
        }
        lines.push(bullet(&tag.key, &tag.value, &tag.display));
        seen.push(key);
    }
    lines
}

fn current_block(context: &TagJobContext) -> String {
    let tags = accepted_conversation(context);
    if tags.is_empty() {
        return "Tags already on this conversation: (none)".to_owned();
    }
    let mut lines = vec![
        "Tags already on this conversation (suggest action \"remove\" only for one of these, and only when the summary shows it is wrong):".to_owned(),
    ];
    for tag in tags {
        lines.push(bullet(&tag.key, &tag.value, &tag.display));
    }
    lines.join("\n")
}

fn accepted_conversation(context: &TagJobContext) -> Vec<&TagOnEntity> {
    context
        .tags
        .iter()
        .filter(|tag| {
            tag.entity_type == TagEntityType::Conversation && tag.state == TagState::Accepted
        })
        .collect()
}

fn bullet(key: &str, value: &str, display: &str) -> String {
    format!("- {}", label(key, value, display))
}

fn with_aliases(line: String, aliases: &[String]) -> String {
    if aliases.is_empty() {
        line
    } else {
        format!("{line} (also: {})", aliases.join(", "))
    }
}

fn label(key: &str, value: &str, display: &str) -> String {
    if display.is_empty() {
        format!("{key}:{value}")
    } else {
        format!("{key}:{display}")
    }
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}
