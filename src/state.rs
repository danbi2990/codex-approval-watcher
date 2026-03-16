use crate::models::{FileState, PersistedState};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub fn load_state(path: &Path) -> PersistedState {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return PersistedState::default(),
    };

    serde_json::from_str::<PersistedState>(&raw).unwrap_or_default()
}

pub fn save_state(path: &Path, state: &PersistedState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create state directory: {}", parent.display())
        })?;
    }

    let temp_path = path.with_extension("tmp");
    let json = serde_json::to_vec_pretty(state).context("failed to serialize watcher state")?;
    fs::write(&temp_path, json)
        .with_context(|| format!("failed to write temp state file: {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to move temp state file into place: {} -> {}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

pub fn prune_deleted_files(state: &mut PersistedState, live_paths: &BTreeSet<String>) {
    state.files = state
        .files
        .iter()
        .filter(|(path, _)| live_paths.contains(*path))
        .map(|(path, file_state)| (path.clone(), file_state.clone()))
        .collect::<BTreeMap<_, _>>();
}

pub fn bootstrap_existing_files(
    state: &mut PersistedState,
    paths_with_meta: &[(String, u64, u64)],
) {
    for (path, mtime_ns, size) in paths_with_meta {
        state.files.insert(
            path.clone(),
            FileState {
                offset: *size,
                cwd: None,
                mtime_ns: *mtime_ns,
                session_id: None,
                seen_calls: Vec::new(),
                size: *size,
            },
        );
    }
    state.initialized = true;
}
