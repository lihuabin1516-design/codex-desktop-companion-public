use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::{json, Value};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub fn product_home() -> PathBuf {
    if let Some(path) = env::var_os("CODEX_PANEFORGE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("CodexPaneForge")
}

pub fn append_event(event: &str, details: Value) -> Result<()> {
    let log_dir = product_home().join("logs");
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("create log directory {}", log_dir.display()))?;
    let path = log_dir.join("companion.jsonl");
    rotate_if_needed(&path)?;
    let payload = json!({
        "schemaVersion": 1,
        "timestampUnixMs": unix_ms(),
        "event": event,
        "details": details,
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open log {}", path.display()))?;
    serde_json::to_writer(&mut file, &payload)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn rotate_if_needed(path: &std::path::Path) -> Result<()> {
    let size = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if size < MAX_LOG_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("jsonl.1");
    if rotated.exists() {
        fs::remove_file(&rotated)
            .with_context(|| format!("remove old rotated log {}", rotated.display()))?;
    }
    fs::rename(path, &rotated).with_context(|| {
        format!(
            "rotate log from {} to {}",
            path.display(),
            rotated.display()
        )
    })?;
    Ok(())
}

pub fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
