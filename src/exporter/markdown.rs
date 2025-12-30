use crate::error::Result;
use crate::providers::base::{ChatMessage, ChatSession, MessageRole};
use chrono::{DateTime, Utc};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Generate markdown content from a chat session
pub fn generate_markdown(session: &ChatSession) -> String {
    let mut md = String::new();

    // Frontmatter
    md.push_str("---\n");
    md.push_str(&format!("provider: {}\n", session.provider));
    md.push_str(&format!("session_id: {}\n", session.session_id));
    md.push_str(&format!("project: {}\n", session.project_path.display()));
    md.push_str(&format!(
        "started_at: {}\n",
        session.started_at.to_rfc3339()
    ));
    md.push_str(&format!(
        "updated_at: {}\n",
        session.updated_at.to_rfc3339()
    ));
    md.push_str(&format!("message_count: {}\n", session.messages.len()));

    // Calculate total tokens if available
    let total_tokens: u32 = session
        .messages
        .iter()
        .filter_map(|m| m.metadata.tokens.as_ref())
        .map(|t| t.input + t.output)
        .sum();

    if total_tokens > 0 {
        md.push_str(&format!("total_tokens: {}\n", total_tokens));
    }

    md.push_str("---\n\n");

    // Title
    let title = extract_title(&session.messages);
    md.push_str(&format!("# {}\n\n", title));

    // Messages
    for message in &session.messages {
        md.push_str(&format_message(message));
        md.push_str("\n\n");
    }

    md
}

/// Format a single message
fn format_message(message: &ChatMessage) -> String {
    let mut md = String::new();

    // Header with role and timestamp
    let role_emoji = match message.role {
        MessageRole::User => "👤",
        MessageRole::Assistant => "🤖",
        MessageRole::System => "⚙️",
    };

    let role_name = match message.role {
        MessageRole::User => "User",
        MessageRole::Assistant => "Assistant",
        MessageRole::System => "System",
    };

    md.push_str(&format!(
        "## {} {} ({})\n\n",
        role_emoji,
        role_name,
        format_datetime(&message.timestamp)
    ));

    // Content
    md.push_str(&message.content);
    md.push('\n');

    // Metadata (if present)
    // Model info removed - not needed in output
    // if let Some(model) = &message.metadata.model {
    //     md.push_str(&format!("\n*Model: {}*\n", model));
    // }

    // Token info removed - not needed in output
    // if let Some(tokens) = &message.metadata.tokens {
    //     md.push_str(&format!(
    //         "\n*Tokens: {} in, {} out, {} cached*\n",
    //         tokens.input, tokens.output, tokens.cached
    //     ));
    // }

    // Tool calls (Claude Code)
    if !message.metadata.tool_calls.is_empty() {
        md.push_str("\n**Tools Used:**\n");
        for tool in &message.metadata.tool_calls {
            md.push_str(&format!("- `{}`\n", tool));
        }
    }

    // Thoughts (Gemini)
    if !message.metadata.thoughts.is_empty() {
        md.push_str("\n<details>\n<summary>💭 Thoughts</summary>\n\n");
        for thought in &message.metadata.thoughts {
            md.push_str(&format!("- {}\n", thought));
        }
        md.push_str("\n</details>\n");
    }

    md
}

/// Extract a title from the first user message
fn extract_title(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .find(|m| matches!(m.role, MessageRole::User))
        .map(|m| {
            // Take first line or first 60 characters
            let first_line = m.content.lines().next().unwrap_or("Untitled Session");
            if first_line.len() > 60 {
                format!("{}...", &first_line[..60])
            } else {
                first_line.to_string()
            }
        })
        .unwrap_or_else(|| "Untitled Session".to_string())
}

/// Format datetime in a human-readable way
fn format_datetime(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// Append new messages to an existing markdown file
pub async fn append_messages(file_path: &Path, messages: &[ChatMessage]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
        .await?;

    for message in messages {
        let content = format_message(message);
        file.write_all(content.as_bytes()).await?;
        file.write_all(b"\n\n").await?;
    }

    file.flush().await?;
    Ok(())
}

/// Create a new markdown file with the full session
pub async fn create_markdown_file(file_path: &Path, session: &ChatSession) -> Result<()> {
    let content = generate_markdown(session);
    fs::write(file_path, content).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title() {
        let messages = vec![ChatMessage {
            id: "1".to_string(),
            timestamp: Utc::now(),
            role: MessageRole::User,
            content: "How do I implement a CLI tool?".to_string(),
            metadata: Default::default(),
        }];

        assert_eq!(extract_title(&messages), "How do I implement a CLI tool?");
    }

    #[test]
    fn test_extract_title_long() {
        let messages = vec![
            ChatMessage {
                id: "1".to_string(),
                timestamp: Utc::now(),
                role: MessageRole::User,
                content: "This is a very long message that should be truncated because it exceeds the maximum length".to_string(),
                metadata: Default::default(),
            },
        ];

        let title = extract_title(&messages);
        assert!(title.len() <= 63); // 60 + "..."
        assert!(title.ends_with("..."));
    }
}
