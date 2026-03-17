use std::{process::Command, thread, time::Duration};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::{config::NotificationsConfig, models::ApprovalEvent};

const LOG_WINDOW_SECONDS: u64 = 10;
const LOG_SETTLE_DELAY: Duration = Duration::from_millis(750);
const LOG_PREDICATE: &str =
    "process == \"osascript\" OR process == \"NotificationCenter\" OR process == \"usernoted\"";
const REQUIRED_LOG_MARKERS: [&str; 4] = [
    "Connection com.apple.ScriptEditor2 with path: /usr/bin/osascript",
    "bundle=com.apple.ScriptEditor2",
    "scheduled for delivery",
    "displaying as banner",
];

#[derive(Debug, Clone, Serialize)]
pub struct NotificationAttempt {
    pub program: String,
    pub args: Vec<String>,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub error: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationDispatchReport {
    pub success: bool,
    pub error: Option<String>,
    pub attempt: NotificationAttempt,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationLogVerification {
    pub predicate: String,
    pub window_seconds: u64,
    pub success: bool,
    pub skipped: bool,
    pub error: Option<String>,
    pub stderr: String,
    pub required_markers: Vec<String>,
    pub missing_markers: Vec<String>,
    pub matched_lines: Vec<String>,
    pub recent_lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationDoctorReport {
    pub dispatch: NotificationDispatchReport,
    pub verification: NotificationLogVerification,
    pub verified: bool,
}

pub fn notify_approval(config: &NotificationsConfig, event: &ApprovalEvent) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    let report = dispatch_notification(config, event);
    if report.success {
        Ok(())
    } else {
        bail!(
            "{}",
            report
                .error
                .as_deref()
                .unwrap_or("notification dispatch failed")
        );
    }
}

pub fn doctor_notification(
    config: &NotificationsConfig,
    event: &ApprovalEvent,
) -> NotificationDoctorReport {
    let dispatch = dispatch_notification(config, event);
    let verification = if dispatch.success {
        thread::sleep(LOG_SETTLE_DELAY);
        verify_notification_logs()
    } else {
        NotificationLogVerification {
            predicate: LOG_PREDICATE.to_string(),
            window_seconds: LOG_WINDOW_SECONDS,
            success: false,
            skipped: true,
            error: Some("dispatch failed before log verification".into()),
            stderr: String::new(),
            required_markers: REQUIRED_LOG_MARKERS
                .iter()
                .map(|marker| marker.to_string())
                .collect(),
            missing_markers: REQUIRED_LOG_MARKERS
                .iter()
                .map(|marker| marker.to_string())
                .collect(),
            matched_lines: Vec::new(),
            recent_lines: Vec::new(),
        }
    };

    let verified = dispatch.success && verification.success;
    NotificationDoctorReport {
        dispatch,
        verification,
        verified,
    }
}

#[cfg(target_os = "macos")]
fn dispatch_notification(
    config: &NotificationsConfig,
    event: &ApprovalEvent,
) -> NotificationDispatchReport {
    let title = "Codex Approval";
    let subtitle = project_name(&event.cwd);
    let attempt = run_osascript(title, &subtitle, &event.message, &config.sound);
    let error = attempt.error.clone();

    NotificationDispatchReport {
        success: attempt.success,
        error,
        attempt,
    }
}

#[cfg(not(target_os = "macos"))]
fn dispatch_notification(
    _config: &NotificationsConfig,
    _event: &ApprovalEvent,
) -> NotificationDispatchReport {
    NotificationDispatchReport {
        success: false,
        error: Some("notifications are only supported on macOS".into()),
        attempt: NotificationAttempt {
            program: "/usr/bin/osascript".into(),
            args: Vec::new(),
            exit_code: None,
            success: false,
            error: Some("notifications are only supported on macOS".into()),
            stdout: String::new(),
            stderr: String::new(),
        },
    }
}

fn project_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(target_os = "macos")]
fn run_osascript(title: &str, subtitle: &str, body: &str, sound: &str) -> NotificationAttempt {
    let program = "/usr/bin/osascript".to_string();
    let args = vec![
        "-e".to_string(),
        applescript_notification(title, subtitle, body, sound),
    ];

    run_command(program, args)
}

#[cfg(target_os = "macos")]
fn run_command(program: String, args: Vec<String>) -> NotificationAttempt {
    let mut command = Command::new(&program);
    command.args(&args);

    match command.output() {
        Ok(output) => {
            let success = output.status.success();
            NotificationAttempt {
                program,
                args,
                exit_code: output.status.code(),
                success,
                error: (!success).then(|| format!("process exited with status {}", output.status)),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
        }
        Err(error) => NotificationAttempt {
            program,
            args,
            exit_code: None,
            success: false,
            error: Some(error.to_string()),
            stdout: String::new(),
            stderr: String::new(),
        },
    }
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

#[cfg(target_os = "macos")]
fn verify_notification_logs() -> NotificationLogVerification {
    let predicate = LOG_PREDICATE.to_string();
    let mut command = Command::new("/usr/bin/log");
    command.args([
        "show",
        "--last",
        &format!("{LOG_WINDOW_SECONDS}s"),
        "--style",
        "compact",
        "--predicate",
        &predicate,
    ]);

    match command.output() {
        Ok(output) => analyze_log_output(
            predicate,
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).as_ref(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        Err(error) => NotificationLogVerification {
            predicate,
            window_seconds: LOG_WINDOW_SECONDS,
            success: false,
            skipped: false,
            error: Some(error.to_string()),
            stderr: String::new(),
            required_markers: REQUIRED_LOG_MARKERS
                .iter()
                .map(|marker| marker.to_string())
                .collect(),
            missing_markers: REQUIRED_LOG_MARKERS
                .iter()
                .map(|marker| marker.to_string())
                .collect(),
            matched_lines: Vec::new(),
            recent_lines: Vec::new(),
        },
    }
}

#[cfg(not(target_os = "macos"))]
fn verify_notification_logs() -> NotificationLogVerification {
    NotificationLogVerification {
        predicate: String::new(),
        window_seconds: LOG_WINDOW_SECONDS,
        success: false,
        skipped: true,
        error: Some("notification log verification is only supported on macOS".into()),
        stderr: String::new(),
        required_markers: Vec::new(),
        missing_markers: Vec::new(),
        matched_lines: Vec::new(),
        recent_lines: Vec::new(),
    }
}

fn analyze_log_output(
    predicate: String,
    command_success: bool,
    stdout: &str,
    stderr: String,
) -> NotificationLogVerification {
    let mut matched_lines = Vec::new();
    let mut missing_markers = Vec::new();

    for marker in REQUIRED_LOG_MARKERS {
        if let Some(line) = stdout.lines().find(|line| line.contains(marker)) {
            matched_lines.push(line.to_string());
        } else {
            missing_markers.push(marker.to_string());
        }
    }

    let recent_lines = stdout
        .lines()
        .filter(|line| {
            line.contains("com.apple.ScriptEditor2")
                || line.contains("scheduled for delivery")
                || line.contains("displaying as banner")
                || line.contains("Presenting <NotificationRecord")
                || line.contains("Connection ")
        })
        .map(|line| line.to_string())
        .take(40)
        .collect::<Vec<_>>();

    let success = command_success && missing_markers.is_empty();
    let error = if !command_success {
        Some("`log show` exited unsuccessfully".into())
    } else if missing_markers.is_empty() {
        None
    } else {
        Some(format!(
            "missing notification delivery markers: {}",
            missing_markers.join(", ")
        ))
    };

    NotificationLogVerification {
        predicate,
        window_seconds: LOG_WINDOW_SECONDS,
        success,
        skipped: false,
        error,
        stderr,
        required_markers: REQUIRED_LOG_MARKERS
            .iter()
            .map(|marker| marker.to_string())
            .collect(),
        missing_markers,
        matched_lines,
        recent_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        REQUIRED_LOG_MARKERS, analyze_log_output, applescript_notification, escape_applescript,
        project_name,
    };

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

    #[test]
    fn analyzes_osascript_log_output() {
        let raw = r#"
2026-03-17 18:25:20.005 Df usernoted[688:2fb75f9] [com.apple.unc:server] Connection com.apple.ScriptEditor2 with path: /usr/bin/osascript
2026-03-17 18:25:20.037 Df usernoted[688:2fb7ce9] [com.apple.usernotificationsd:NotificationsPipeline] [create, [id=DA39-A3EE, time=2026-03-17 09:25:20, bundle=com.apple.ScriptEditor2], Time elapsed=0.024 sec]: Request: Calling pipeline completion with success
2026-03-17 18:25:20.037 Df usernoted[688:2fb7ce9] [com.apple.unc:application] <NotificationRecord app:"com.apple.ScriptEditor2" ident:"DA39-A3EE" req:"" uuid:"EF582C36" source:"3EC3C5E7" staticCategory:"<LEGACY options=(legacyBehavior, hiddenPreviewShowsTitle) actions=[]>"> successfully processed by pipeline, scheduled for delivery.
2026-03-17 18:25:20.142 Df usernoted[688:2fb7cea] [com.apple.unc:application] Presenting <NotificationRecord app:"com.apple.ScriptEditor2" ident:"DA39-A3EE"> as banner (["badge", "sound", "alert"])
2026-03-17 18:25:20.143 Df NotificationCenter[677:194e] [com.apple.unc:application] [com.apple.ScriptEditor2:EF582C36][record: <NotificationRecord app:"com.apple.ScriptEditor2" ident:"DA39-A3EE" req:"" uuid:"EF582C36" source:"3EC3C5E7">] displaying as banner, body: <private>, summary: <private>
"#;

        let report = analyze_log_output("predicate".into(), true, raw, String::new());

        assert!(report.success);
        assert!(report.missing_markers.is_empty());
        assert_eq!(
            report.required_markers,
            REQUIRED_LOG_MARKERS
                .iter()
                .map(|marker| marker.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn reports_missing_log_markers() {
        let report = analyze_log_output(
            "predicate".into(),
            true,
            "Connection com.apple.ScriptEditor2 with path: /usr/bin/osascript",
            String::new(),
        );

        assert!(!report.success);
        assert_eq!(
            report.missing_markers,
            vec![
                "bundle=com.apple.ScriptEditor2".to_string(),
                "scheduled for delivery".to_string(),
                "displaying as banner".to_string(),
            ]
        );
    }
}
