use crate::{config::NotificationsConfig, models::ApprovalEvent};
use anyhow::Result;

pub fn notify_approval(config: &NotificationsConfig, event: &ApprovalEvent) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    platform_notify(config, event)
}

#[cfg(target_os = "macos")]
fn platform_notify(config: &NotificationsConfig, event: &ApprovalEvent) -> Result<()> {
    use anyhow::Context;
    use mac_notification_sys::{
        Notification, get_bundle_identifier_or_default, send_notification, set_application,
    };

    let bundle = get_bundle_identifier_or_default(&config.app);
    set_application(&bundle).context("failed to set macOS notification application")?;

    let subtitle = project_name(&event.cwd);
    let mut notification = Notification::new();
    notification.sound(&config.sound);

    send_notification(
        "Codex Approval",
        if subtitle.is_empty() {
            None
        } else {
            Some(subtitle.as_str())
        },
        &event.message,
        Some(&notification),
    )
    .context("failed to send macOS notification")?;

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

#[cfg(test)]
mod tests {
    use super::project_name;

    #[test]
    fn extracts_project_name_from_cwd() {
        assert_eq!(project_name("/Users/jake/project-x"), "project-x");
        assert_eq!(project_name("/Users/jake/.codex"), ".codex");
        assert_eq!(project_name("/"), "");
    }
}
