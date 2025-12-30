use crate::error::{Result, WaylogError};
use crate::providers::base::*;
use crate::utils::path;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct ClaudeProvider;

impl ClaudeProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn name(&self) -> &str {
        "claude"
    }

    fn data_dir(&self) -> Result<PathBuf> {
        path::get_ai_data_dir("claude").map(|p| p.join("projects"))
    }

    fn session_dir(&self, project_path: &Path) -> Result<PathBuf> {
        let encoded = path::encode_path_claude(project_path);
        Ok(self.data_dir()?.join(encoded))
    }

    async fn find_latest_session(&self, project_path: &Path) -> Result<Option<PathBuf>> {
        let candidates = self.get_all_sessions(project_path).await?;
        Ok(candidates.into_iter().next())
    }

    async fn get_all_sessions(&self, project_path: &Path) -> Result<Vec<PathBuf>> {
        let session_dir = self.session_dir(project_path)?;

        if !session_dir.exists() {
            return Ok(Vec::new());
        }

        // Find all .jsonl files
        let mut entries = fs::read_dir(&session_dir).await?;
        let mut candidates = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                // Filter main sessions
                if self.is_main_session(&path).await.unwrap_or(false) {
                    let metadata = fs::metadata(&path).await?;
                    let modified = metadata.modified()?;
                    candidates.push((path, modified));
                }
            }
        }

        // Sort by modification time, newest first
        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(candidates.into_iter().map(|(p, _)| p).collect())
    }

    async fn parse_session(&self, file_path: &Path) -> Result<ChatSession> {
        let file = fs::File::open(file_path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let mut messages = Vec::new();
        let mut session_id = String::new();
        let mut started_at = Utc::now();
        let mut project_path = PathBuf::new();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            let event: ClaudeEvent = serde_json::from_str(&line).map_err(WaylogError::Json)?;

            // Extract session metadata from first event
            if session_id.is_empty() {
                session_id = event.session_id.clone().unwrap_or_else(|| {
                    file_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string()
                });

                if let Some(cwd) = &event.cwd {
                    project_path = PathBuf::from(cwd);
                }
            }

            // Parse user and assistant messages
            if event.event_type == "user" || event.event_type == "assistant" {
                if let Some(msg) = self.parse_message(event)? {
                    if messages.is_empty() {
                        started_at = msg.timestamp;
                    }
                    messages.push(msg);
                }
            }
        }

        Ok(ChatSession {
            session_id,
            provider: self.name().to_string(),
            project_path,
            started_at,
            updated_at: messages.last().map(|m| m.timestamp).unwrap_or(started_at),
            messages,
        })
    }

    fn is_installed(&self) -> bool {
        which::which("claude").is_ok()
    }

    fn command(&self) -> &str {
        "claude"
    }
}

impl ClaudeProvider {
    fn parse_message(&self, event: ClaudeEvent) -> Result<Option<ChatMessage>> {
        let role = match event.event_type.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => return Ok(None),
        };

        // Extract content from message
        let content = match &event.message {
            Some(msg) => match &msg.content {
                ClaudeContent::Text(text) => text.clone(),
                ClaudeContent::Array(items) => items
                    .iter()
                    .filter_map(|item| {
                        if item.content_type == "text" {
                            item.text.clone()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            None => return Ok(None),
        };

        if content.is_empty() {
            return Ok(None);
        }

        // Format XML content to look like official export
        let content = if role == MessageRole::User {
            Self::format_claude_xml(&content)
        } else {
            content
        };

        let timestamp = event
            .timestamp
            .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        // Extract metadata
        let (model, tokens, tool_calls) = if let Some(msg) = &event.message {
            let model = msg.model.clone();
            let tokens = msg.usage.as_ref().map(|u| TokenUsage {
                input: u.input_tokens,
                output: u.output_tokens,
                cached: u.cache_read_input_tokens.unwrap_or(0),
            });

            // Extract tool calls
            let tool_calls = if let ClaudeContent::Array(items) = &msg.content {
                items
                    .iter()
                    .filter(|item| item.content_type == "tool_use")
                    .filter_map(|item| item.name.clone())
                    .collect()
            } else {
                Vec::new()
            };

            (model, tokens, tool_calls)
        } else {
            (None, None, Vec::new())
        };

        Ok(Some(ChatMessage {
            id: event
                .uuid
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            timestamp,
            role,
            content,
            metadata: MessageMetadata {
                model,
                tokens,
                tool_calls,
                thoughts: Vec::new(),
            },
        }))
    }

    /// Format Claude Code XML tags into markdown-friendly text
    fn format_claude_xml(content: &str) -> String {
        // Handle Command Name: <command-name>cmd</command-name>
        if let Some(start) = content.find("<command-name>") {
            if let Some(end) = content[start..].find("</command-name>") {
                let cmd = &content[start + 14..start + end];

                // Only format if command starts with slash (e.g. /resume)
                // This preserves user input like "<command-name>My Custom Command</command-name>"
                if cmd.trim().starts_with('/') {
                    return format!("> {}", cmd.trim());
                }
            }
        }

        // Handle Stdout: <local-command-stdout>output</local-command-stdout>
        if let Some(start) = content.find("<local-command-stdout>") {
            if let Some(end) = content[start..].find("</local-command-stdout>") {
                let out = &content[start + 22..start + end];
                return format!("> ⎿ {}", out.trim());
            }
        }

        content.to_string()
    }

    /// Check if a session file is a main session (not a sidechain)
    async fn is_main_session(&self, path: &Path) -> Result<bool> {
        let file = fs::File::open(path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let mut checked_lines = 0;
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            // Limit checks to first 10 lines
            if checked_lines >= 10 {
                break;
            }
            checked_lines += 1;

            // Fast path: simple string check
            if line.contains("\"isSidechain\":true") {
                return Ok(false);
            }
            if line.contains("\"isSidechain\":false") {
                return Ok(true);
            }

            // Precise path: JSON parsing
            if let Ok(event) = serde_json::from_str::<ClaudeEvent>(&line) {
                if let Some(true) = event.is_sidechain {
                    return Ok(false);
                }
            }
        }

        // Default to true if not specified
        Ok(true)
    }
}

// Claude Code JSONL event structures
#[derive(Debug, Deserialize)]
struct ClaudeEvent {
    #[serde(rename = "type")]
    event_type: String,

    #[serde(rename = "sessionId")]
    session_id: Option<String>,

    cwd: Option<String>,
    timestamp: Option<String>,
    uuid: Option<String>,

    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,

    message: Option<ClaudeMessage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    #[allow(dead_code)]
    role: String,
    content: ClaudeContent,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ClaudeContent {
    Text(String),
    Array(Vec<ClaudeContentItem>),
}

#[derive(Debug, Deserialize)]
struct ClaudeContentItem {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
    name: Option<String>, // For tool_use
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: u32,
    output_tokens: u32,
    cache_read_input_tokens: Option<u32>,
}
