use crate::error::Result;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone)]
pub struct Frontmatter {
    pub session_id: Option<String>,
    pub provider: Option<String>,
    pub message_count: Option<usize>,
}

/// Parse minimal frontmatter from a markdown file
pub async fn parse_frontmatter(path: &Path) -> Result<Frontmatter> {
    let mut file = fs::File::open(path).await?;

    // Read first 2KB which should cover the frontmatter
    let mut buffer = [0u8; 2048];
    let n = file.read(&mut buffer).await?;
    let content = String::from_utf8_lossy(&buffer[..n]);

    let mut fm = Frontmatter {
        session_id: None,
        provider: None,
        message_count: None,
    };

    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end_idx) = stripped.find("---") {
            let yaml_block = &stripped[..end_idx];

            for line in yaml_block.lines() {
                let line = line.trim();

                if let Some(val) = line.strip_prefix("session_id:") {
                    fm.session_id = Some(val.trim().to_string());
                } else if let Some(val) = line.strip_prefix("provider:") {
                    fm.provider = Some(val.trim().to_string());
                } else if let Some(val) = line.strip_prefix("message_count:") {
                    if let Ok(count) = val.trim().parse() {
                        fm.message_count = Some(count);
                    }
                }
            }
        }
    }

    Ok(fm)
}
