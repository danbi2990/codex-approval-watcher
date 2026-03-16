use std::process::Command;

use anyhow::{Context, Result};

use crate::{config::NotificationsConfig, models::ApprovalEvent};

pub fn notify_approval(config: &NotificationsConfig, event: &ApprovalEvent) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    platform_notify(config, event)
}

#[cfg(target_os = "macos")]
fn platform_notify(config: &NotificationsConfig, event: &ApprovalEvent) -> Result<()> {
    let title = "Codex Approval";
    let subtitle = project_name(&event.cwd);

    if try_terminal_notifier(title, &subtitle, &event.message, &config.sound)? {
        return Ok(());
    }

    let script = applescript_notification(title, &subtitle, &event.message, &config.sound);
    let status = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status()
        .context("failed to launch osascript for notification")?;
    if !status.success() {
        anyhow::bail!("osascript notification exited with status {status}");
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn platform_notify(_config: &NotificationsConfig, _event: &ApprovalEvent) -> Result<()> {
    Ok(())
}

fn project_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(target_os = "macos")]
fn try_terminal_notifier(title: &str, subtitle: &str, body: &str, sound: &str) -> Result<bool> {
    let mut command = Command::new("terminal-notifier");
    for arg in terminal_notifier_args(title, subtitle, body, sound) {
        command.arg(arg);
    }

    match command.status() {
        Ok(status) => Ok(status.success()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("failed to launch terminal-notifier"),
    }
}

#[cfg(target_os = "macos")]
fn terminal_notifier_args<'a>(
    title: &'a str,
    subtitle: &'a str,
    body: &'a str,
    sound: &'a str,
) -> Vec<&'a str> {
    let mut args = vec!["-title", title, "-message", body];

    if !sound.trim().is_empty() {
        args.extend(["-sound", sound]);
    }

    if !subtitle.trim().is_empty() {
        args.extend(["-subtitle", subtitle]);
    }

    args
}

#[cfg(target_os = "macos")]
fn applescript_notification(title: &str, subtitle: &str, body: &str, sound: &str) -> String {
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(title)
    );

    if !subtitle.trim().is_empty() {
        script.push_str(&format!(" subtitle \"{}\"", escape_applescript(subtitle)));
    }

    if !sound.trim().is_empty() {
        script.push_str(&format!(" sound name \"{}\"", escape_applescript(sound)));
    }

    script
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::project_name;
    #[cfg(target_os = "macos")]
    use super::{applescript_notification, escape_applescript, terminal_notifier_args};

    #[test]
    fn extracts_project_name_from_cwd() {
        assert_eq!(project_name("/Users/tester/project-x"), "project-x");
        assert_eq!(project_name("/Users/tester/.codex"), ".codex");
        assert_eq!(project_name("/"), "");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escapes_applescript_strings() {
        assert_eq!(escape_applescript("a\\b\"c"), "a\\\\b\\\"c");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn builds_applescript_notification() {
        let script = applescript_notification("Title", "project-x", "Need approval", "Sosumi");
        assert!(script.contains("display notification"));
        assert!(script.contains("with title \"Title\""));
        assert!(script.contains("subtitle \"project-x\""));
        assert!(script.contains("sound name \"Sosumi\""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn builds_terminal_notifier_args() {
        let args = terminal_notifier_args("Title", "project-x", "Need approval", "Sosumi");
        assert_eq!(
            args,
            vec![
                "-title",
                "Title",
                "-message",
                "Need approval",
                "-sound",
                "Sosumi",
                "-subtitle",
                "project-x",
            ]
        );
    }
}
