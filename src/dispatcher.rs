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
