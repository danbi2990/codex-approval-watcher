use serde::Deserialize;
use std::{env, path::PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub sessions_root: PathBuf,
    pub state_file: PathBuf,
    #[serde(default = "default_event_timeout_ms")]
    pub event_timeout_ms: u64,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotificationsConfig {
    #[serde(default = "default_notifications_enabled")]
    pub enabled: bool,
    #[serde(default = "default_notification_app")]
    pub app: String,
    #[serde(default = "default_notification_sound")]
    pub sound: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default = "default_hook_timeout_ms")]
    pub timeout_ms: u64,
}

const fn default_event_timeout_ms() -> u64 {
    1000
}

const fn default_hook_timeout_ms() -> u64 {
    3000
}

const fn default_notifications_enabled() -> bool {
    true
}

fn default_notification_app() -> String {
    "Code".to_string()
}

fn default_notification_sound() -> String {
    "Sosumi".to_string()
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: default_notifications_enabled(),
            app: default_notification_app(),
            sound: default_notification_sound(),
        }
    }
}

pub fn expand_config_paths(mut config: Config) -> Config {
    config.sessions_root = expand_path(&config.sessions_root);
    config.state_file = expand_path(&config.state_file);

    for hook in &mut config.hooks {
        if let Some(first) = hook.command.first_mut() {
            let expanded = expand_path(&PathBuf::from(first.as_str()));
            *first = expanded.display().to_string();
        }
    }

    config
}

fn expand_path(path: &PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return home_dir().unwrap_or_else(|| path.clone());
    }

    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    }

    path.clone()
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
