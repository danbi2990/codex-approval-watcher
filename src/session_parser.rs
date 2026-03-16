use crate::models::{ApprovalEvent, FileState};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

const MAX_SEEN_CALLS: usize = 100;

pub fn current_file_signature(path: &Path) -> Result<(u64, u64)> {
    let metadata = path
        .metadata()
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    Ok((mtime_ns, metadata.len()))
}

pub fn hydrate_session_metadata(path: &Path, file_state: &mut FileState) -> Result<()> {
    if file_state
        .cwd
        .as_ref()
        .is_some_and(|cwd| !cwd.is_empty())
        && file_state
            .session_id
            .as_ref()
            .is_some_and(|id| !id.is_empty())
    {
        return Ok(());
    }

    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line.with_context(|| format!("failed to read {}", path.display()))?;
        let Some(entry) = parse_json_line(&line) else {
            continue;
        };

        let Some(entry_type) = entry.get("type").and_then(Value::as_str) else {
            continue;
        };

        if entry_type != "session_meta" {
            continue;
        }

        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };

        if let Some(cwd) = payload
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            file_state.cwd = Some(cwd.to_string());
        }

        if let Some(session_id) = payload
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            file_state.session_id = Some(session_id.to_string());
        }

        if file_state.cwd.is_some() && file_state.session_id.is_some() {
            return Ok(());
        }
    }

    Ok(())
}

pub fn process_file(path: &Path, file_state: &mut FileState) -> Result<Vec<ApprovalEvent>> {
    let (mtime_ns, file_size) = current_file_signature(path)?;
    let metadata_missing = file_state.cwd.as_deref().unwrap_or("").is_empty()
        || file_state.session_id.as_deref().unwrap_or("").is_empty();

    if !metadata_missing && mtime_ns == file_state.mtime_ns && file_size == file_state.size {
        return Ok(Vec::new());
    }

    hydrate_session_metadata(path, file_state)?;

    let mut offset = file_state.offset;
    if file_size < offset {
        offset = 0;
    }

    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if bytes == 0 {
            break;
        }

        if let Some(event) = handle_entry(line.trim_end_matches('\n'), file_state)? {
            events.push(event);
        }
    }

    file_state.offset = reader.stream_position().unwrap_or(file_size);
    file_state.mtime_ns = mtime_ns;
    file_state.size = file_size;

    Ok(events)
}

fn handle_entry(line: &str, file_state: &mut FileState) -> Result<Option<ApprovalEvent>> {
    let Some(entry) = parse_json_line(line) else {
        return Ok(None);
    };

    let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = entry.get("payload");

    if entry_type == "session_meta" {
        if let Some(payload) = payload.and_then(Value::as_object) {
            if let Some(cwd) = payload
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                file_state.cwd = Some(cwd.to_string());
            }
            if let Some(session_id) = payload
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                file_state.session_id = Some(session_id.to_string());
            }
        }
        return Ok(None);
    }

    if entry_type != "response_item" {
        return Ok(None);
    }

    let Some(payload) = payload.and_then(Value::as_object) else {
        return Ok(None);
    };

    if payload.get("type").and_then(Value::as_str) != Some("function_call") {
        return Ok(None);
    }

    let Some(arguments_raw) = payload.get("arguments").and_then(Value::as_str) else {
        return Ok(None);
    };
    let arguments: Value = match serde_json::from_str(arguments_raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    if arguments
        .get("sandbox_permissions")
        .and_then(Value::as_str)
        != Some("require_escalated")
    {
        return Ok(None);
    }

    let Some(cwd) = file_state.cwd.clone().filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(session_id) = file_state
        .session_id
        .clone()
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let call_id = payload
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                entry.get("timestamp").and_then(Value::as_str).unwrap_or(""),
                payload.get("name").and_then(Value::as_str).unwrap_or(""),
                arguments.get("cmd").and_then(Value::as_str).unwrap_or("")
            )
        });

    if file_state.seen_calls.iter().any(|seen| seen == &call_id) {
        return Ok(None);
    }

    let message = arguments
        .get("justification")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Approval required")
        .to_string();

    let command = arguments
        .get("cmd")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Command requires approval")
        .to_string();

    let timestamp = entry
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    file_state.seen_calls.push(call_id);
    if file_state.seen_calls.len() > MAX_SEEN_CALLS {
        let drain_count = file_state.seen_calls.len() - MAX_SEEN_CALLS;
        file_state.seen_calls.drain(0..drain_count);
    }

    Ok(Some(ApprovalEvent {
        event: "approval.requested",
        session_id,
        cwd,
        timestamp,
        message,
        command,
    }))
}

fn parse_json_line(line: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(map)) => Some(Value::Object(map)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{hydrate_session_metadata, process_file};
    use crate::models::FileState;
    use std::fs;

    #[test]
    fn hydrates_metadata_from_session_meta() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.jsonl");
        fs::write(
            &path,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp/project-a\"}}\n",
        )
        .unwrap();

        let mut state = FileState::default();
        hydrate_session_metadata(&path, &mut state).unwrap();

        assert_eq!(state.session_id.as_deref(), Some("sess-1"));
        assert_eq!(state.cwd.as_deref(), Some("/tmp/project-a"));
    }

    #[test]
    fn process_file_emits_approval_event() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-16T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp/project-a\"}}\n",
                "{\"timestamp\":\"2026-03-16T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"printf hi > /tmp/a\\\",\\\"justification\\\":\\\"Need approval\\\",\\\"sandbox_permissions\\\":\\\"require_escalated\\\"}\",\"call_id\":\"call-1\"}}\n"
            ),
        )
        .unwrap();

        let mut state = FileState::default();
        let events = process_file(&path, &mut state).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "approval.requested");
        assert_eq!(events[0].session_id, "sess-1");
        assert_eq!(events[0].cwd, "/tmp/project-a");
        assert_eq!(events[0].message, "Need approval");
        assert_eq!(events[0].command, "printf hi > /tmp/a");
    }

    #[test]
    fn process_file_dedupes_seen_call_ids() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-16T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp/project-a\"}}\n",
                "{\"timestamp\":\"2026-03-16T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"printf hi > /tmp/a\\\",\\\"justification\\\":\\\"Need approval\\\",\\\"sandbox_permissions\\\":\\\"require_escalated\\\"}\",\"call_id\":\"call-1\"}}\n"
            ),
        )
        .unwrap();

        let mut state = FileState::default();
        let first = process_file(&path, &mut state).unwrap();
        let second = process_file(&path, &mut state).unwrap();

        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
    }

    #[test]
    fn process_file_handles_truncation_and_new_event() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.jsonl");
        fs::write(
            &path,
            "{\"timestamp\":\"2026-03-16T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp/project-a\"}}\n",
        )
        .unwrap();

        let mut state = FileState::default();
        let first = process_file(&path, &mut state).unwrap();
        assert!(first.is_empty());

        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-16T00:00:02Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp/project-a\"}}\n",
                "{\"timestamp\":\"2026-03-16T00:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"printf hi > /tmp/b\\\",\\\"justification\\\":\\\"Need approval again\\\",\\\"sandbox_permissions\\\":\\\"require_escalated\\\"}\",\"call_id\":\"call-2\"}}\n"
            ),
        )
        .unwrap();

        let second = process_file(&path, &mut state).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].command, "printf hi > /tmp/b");
    }
}
