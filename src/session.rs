use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const PROJECT_QUALIFIER: &str = "works";
const PROJECT_ORGANIZATION: &str = "higgs";
const PROJECT_APPLICATION: &str = "cduo";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub session_name: String,
    pub project_name: String,
    pub display_name: String,
    pub cwd: PathBuf,
    pub created_at: String,
    pub agent: String,
    pub mode: Option<String>,
    pub hook_port: u16,
    pub panes: HashMap<String, PaneMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneMetadata {
    pub pane_id: String,
    pub attach_port: u16,
}

pub fn get_state_root() -> PathBuf {
    if let Ok(dir) = std::env::var("CDUO_STATE_DIR") {
        return PathBuf::from(dir);
    }

    if let Some(proj_dirs) = ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORGANIZATION, PROJECT_APPLICATION) {
        return proj_dirs.state_dir().map(Path::to_path_buf).unwrap_or_else(|| {
            proj_dirs.data_dir().to_path_buf()
        });
    }

    PathBuf::from("/tmp/cduo-state")
}

pub fn get_session_root() -> PathBuf {
    get_state_root().join("sessions")
}

pub fn ensure_session_root() -> Result<PathBuf> {
    let root = get_session_root();
    fs::create_dir_all(&root)?;
    Ok(root)
}

pub fn get_session_dir(session_id: &str) -> PathBuf {
    get_session_root().join(session_id)
}

pub fn write_session_metadata(session_id: &str, metadata: &SessionMetadata) -> Result<()> {
    let dir = get_session_dir(session_id);
    fs::create_dir_all(&dir)?;
    let path = dir.join("session.json");
    let temp = dir.join(format!(".session.json.{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_string_pretty(metadata)?)?;
    fs::rename(&temp, &path)?;
    Ok(())
}

pub fn read_session_metadata(session_id: &str) -> Result<Option<SessionMetadata>> {
    let path = get_session_dir(session_id).join("session.json");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&content).ok())
}

pub fn list_sessions() -> Result<Vec<(String, Option<SessionMetadata>)>> {
    let root = ensure_session_root()?;
    let mut sessions = Vec::new();

    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = read_session_metadata(&name)?;
            sessions.push((name, meta));
        }
    }

    Ok(sessions)
}

pub fn remove_session(session_id: &str) -> Result<()> {
    let dir = get_session_dir(session_id);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    Ok(())
}
