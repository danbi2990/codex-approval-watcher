use crate::models::{FileState, PersistedState};
use anyhow::{Context, Result};
use std::{collections::BTreeSet, fs, path::Path};

pub fn load_state(path: &Path) -> PersistedState {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return PersistedState::default(),
    };

    serde_json::from_str::<PersistedState>(&raw).unwrap_or_default()
}

pub fn save_state(path: &Path, state: &PersistedState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create state directory: {}", parent.display()))?;
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

pub fn prune_deleted_files(state: &mut PersistedState, live_paths: &BTreeSet<String>) -> bool {
    let initial_len = state.files.len();
    state.files.retain(|path, _| live_paths.contains(path));
    state.files.len() != initial_len
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

#[cfg(test)]
mod tests {
    use super::{bootstrap_existing_files, prune_deleted_files};
    use crate::models::{FileState, PersistedState};
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn bootstrap_marks_existing_files_initialized() {
        let mut state = PersistedState::default();
        bootstrap_existing_files(
            &mut state,
            &[
                ("/tmp/a.jsonl".into(), 11, 42),
                ("/tmp/b.jsonl".into(), 22, 84),
            ],
        );

        assert!(state.initialized);
        assert_eq!(state.files["/tmp/a.jsonl"].offset, 42);
        assert_eq!(state.files["/tmp/a.jsonl"].mtime_ns, 11);
        assert_eq!(state.files["/tmp/b.jsonl"].size, 84);
    }

    #[test]
    fn prune_deleted_files_removes_missing_entries() {
        let mut state = PersistedState {
            initialized: true,
            files: BTreeMap::from([
                ("/tmp/a.jsonl".into(), FileState::default()),
                ("/tmp/b.jsonl".into(), FileState::default()),
            ]),
        };
        let live = BTreeSet::from(["/tmp/b.jsonl".to_string()]);

        let changed = prune_deleted_files(&mut state, &live);

        assert!(changed);
        assert!(!state.files.contains_key("/tmp/a.jsonl"));
        assert!(state.files.contains_key("/tmp/b.jsonl"));
    }
}
