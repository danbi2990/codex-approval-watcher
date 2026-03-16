use crate::{config::HookConfig, models::ApprovalEvent};
use anyhow::{Context, Result, bail};
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn dispatch_event(hooks: &[HookConfig], event: &ApprovalEvent) -> Result<()> {
    let payload = serde_json::to_vec(event).context("failed to serialize approval event")?;

    for hook in hooks {
        dispatch_to_hook(hook, &payload, event)?;
    }

    Ok(())
}

fn dispatch_to_hook(hook: &HookConfig, payload: &[u8], event: &ApprovalEvent) -> Result<()> {
    if hook.command.is_empty() {
        return Ok(());
    }

    let mut command = Command::new(&hook.command[0]);
    if hook.command.len() > 1 {
        command.args(&hook.command[1..]);
    }

    command
        .env("CODEX_EVENT_TYPE", event.event)
        .env("CODEX_SESSION_ID", &event.session_id)
        .env("CODEX_CWD", &event.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn hook `{}`", hook.name))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(payload)
            .with_context(|| format!("failed to write stdin for hook `{}`", hook.name))?;
    }

    let timeout = Duration::from_millis(hook.timeout_ms);
    let start = Instant::now();

    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("failed to wait for hook `{}`", hook.name))?
        {
            if status.success() {
                return Ok(());
            }

            bail!("hook `{}` exited with status {}", hook.name, status);
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("hook `{}` timed out after {}ms", hook.name, hook.timeout_ms);
        }

        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch_event;
    use crate::{
        config::HookConfig,
        models::ApprovalEvent,
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn dispatch_event_writes_json_to_hook_stdin() {
        let temp = tempfile::tempdir().unwrap();
        let output_path = temp.path().join("event.json");
        let hook_path = temp.path().join("hook.sh");
        fs::write(
            &hook_path,
            format!(
                "#!/bin/zsh\ncat > {}\n",
                output_path.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook_path, permissions).unwrap();

        let hook = HookConfig {
            name: "capture".into(),
            command: vec![hook_path.display().to_string()],
            timeout_ms: 1000,
        };
        let event = ApprovalEvent {
            event: "approval.requested",
            session_id: "sess-1".into(),
            cwd: "/tmp/project-a".into(),
            timestamp: "2026-03-16T00:00:00Z".into(),
            message: "Need approval".into(),
            command: "printf hi > /tmp/a".into(),
        };

        dispatch_event(&[hook], &event).unwrap();

        let written = fs::read_to_string(output_path).unwrap();
        assert!(written.contains("\"event\":\"approval.requested\""));
        assert!(written.contains("\"session_id\":\"sess-1\""));
    }
}
