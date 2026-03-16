use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalEvent {
    pub event: &'static str,
    pub session_id: String,
    pub cwd: String,
    pub timestamp: String,
    pub message: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedState {
    #[serde(default)]
    pub initialized: bool,
    #[serde(default)]
    pub files: BTreeMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub offset: u64,
    pub cwd: Option<String>,
    pub mtime_ns: u64,
    pub session_id: Option<String>,
    #[serde(default)]
    pub seen_calls: Vec<String>,
    pub size: u64,
}

impl Default for FileState {
    fn default() -> Self {
        Self {
            offset: 0,
            cwd: None,
            mtime_ns: 0,
            session_id: None,
            seen_calls: Vec::new(),
            size: 0,
        }
    }
}
