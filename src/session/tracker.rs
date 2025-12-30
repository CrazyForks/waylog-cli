use crate::error::Result;
use crate::providers::base::{ChatSession, Provider};
use crate::session::state::{ProjectState, SessionState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;

/// Session tracker - manages active sessions and their sync state
pub struct SessionTracker {
    project_dir: PathBuf,
    provider: Arc<dyn Provider>,
    state: Arc<Mutex<ProjectState>>,
}

impl SessionTracker {
    /// Create a new session tracker
    /// Create a new session tracker
    pub async fn new(project_dir: PathBuf, provider: Arc<dyn Provider>) -> Result<Self> {
        // Start with empty state (stateless design)
        let state = ProjectState {
            sessions: std::collections::HashMap::new(),
        };

        let tracker = Self {
            project_dir,
            provider,
            state: Arc::new(Mutex::new(state)),
        };

        // Restore state from existing markdown files
        tracker.restore_from_disk().await?;

        Ok(tracker)
    }

    /// Get the current sync state
    pub async fn get_state(&self) -> ProjectState {
        self.state.lock().await.clone()
    }

    /// Save the current state to disk
    pub async fn save_state(&self) -> Result<()> {
        // Persistence disabled: Markdown files are the source of truth
        Ok(())
    }

    /// Get the number of synced messages for a session
    pub async fn get_synced_count(&self, session_id: &str) -> usize {
        let state = self.state.lock().await;
        state.get_synced_count(session_id)
    }

    /// Get the existing markdown path for a session if it exists
    pub async fn get_markdown_path(&self, session_id: &str) -> Option<PathBuf> {
        let state = self.state.lock().await;
        state
            .sessions
            .get(session_id)
            .map(|s| s.markdown_path.clone())
    }

    /// Update session state after syncing
    pub async fn update_session(
        &self,
        session_id: String,
        file_path: PathBuf,
        markdown_path: PathBuf,
        synced_count: usize,
    ) -> Result<()> {
        let mut state = self.state.lock().await;

        let session_state = SessionState {
            session_id: session_id.clone(),
            provider: self.provider.name().to_string(),
            file_path,
            markdown_path,
            synced_message_count: synced_count,
            last_sync_time: chrono::Utc::now(),
        };

        state.upsert_session(session_state);

        // Persistence disabled
        Ok(())
    }

    /// Process a session file and return new messages
    pub async fn get_new_messages(
        &self,
        file_path: &Path,
    ) -> Result<(ChatSession, Vec<crate::providers::base::ChatMessage>)> {
        // Parse the session
        let session = self.provider.parse_session(file_path).await?;

        // Get the number of already synced messages
        let synced_count = self.get_synced_count(&session.session_id).await;

        // Get new messages
        let new_messages = session
            .messages
            .iter()
            .skip(synced_count)
            .cloned()
            .collect();

        Ok((session, new_messages))
    }

    /// Scan markdown files to restore session state
    async fn restore_from_disk(&self) -> Result<()> {
        let history_dir = crate::utils::path::get_waylog_dir(&self.project_dir);
        if !history_dir.exists() {
            return Ok(());
        }

        // Read directory
        let mut entries = match fs::read_dir(&history_dir).await {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        let mut sessions_map = std::collections::HashMap::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                // Try to parse frontmatter
                if let Ok(fm) = crate::exporter::parse_frontmatter(&path).await {
                    if let Some(sid) = fm.session_id {
                        let state = SessionState {
                            session_id: sid.clone(),
                            provider: fm
                                .provider
                                .unwrap_or_else(|| self.provider.name().to_string()),
                            file_path: PathBuf::new(), // Unknown source path
                            markdown_path: path.clone(),
                            synced_message_count: fm.message_count.unwrap_or(0),
                            last_sync_time: chrono::Utc::now(), // Unknown
                        };
                        sessions_map.insert(sid, state);
                    }
                }
            }
        }

        // Update state if we found anything
        if !sessions_map.is_empty() {
            let mut state = self.state.lock().await;
            state.sessions = sessions_map;
        }

        Ok(())
    }
}
