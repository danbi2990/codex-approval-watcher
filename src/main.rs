mod config;
mod dispatcher;
mod models;
mod notifier;
mod session_parser;
mod state;

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use config::{Config, expand_config_paths};
use dispatcher::dispatch_event;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use models::{FileState, PersistedState};
use notifier::notify_approval;
use session_parser::{current_file_signature, process_file};
use state::{bootstrap_existing_files, load_state, prune_deleted_files, save_state};
use walkdir::WalkDir;

const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");
static RUNNING: AtomicBool = AtomicBool::new(true);

fn main() -> Result<()> {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        Some("run") => {
            let config_path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("config.toml"));
            let config = load_config(&config_path)?;
            validate_config(&config)?;
            run(&config)
        }
        Some("print-example-config") => {
            print!("{EXAMPLE_CONFIG}");
            Ok(())
        }
        Some("validate-config") => {
            let config_path = args
                .next()
                .context("usage: codex-approval-watcher validate-config <path>")?;
            let config = load_config(Path::new(&config_path))?;
            validate_config(&config)?;
            println!("Config is valid: {}", Path::new(&config_path).display());
            Ok(())
        }
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(command) => {
            bail!("unknown command: {command}");
        }
        None => {
            print_help();
            Ok(())
        }
    }
}

fn load_config(path: &Path) -> Result<Config> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    let config: Config = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config: {}", path.display()))?;
    Ok(expand_config_paths(config))
}

fn validate_config(config: &Config) -> Result<()> {
    for hook in &config.hooks {
        if hook.name.trim().is_empty() {
            bail!("hook name must not be empty");
        }

        if hook.command.is_empty() {
            bail!("hook `{}` must provide a command", hook.name);
        }

        if hook.timeout_ms == 0 {
            bail!(
                "hook `{}` must have timeout_ms greater than zero",
                hook.name
            );
        }
    }

    if !config.notifications.enabled && config.hooks.is_empty() {
        bail!("config must enable notifications or contain at least one hook");
    }

    if config.event_timeout_ms == 0 {
        bail!("event_timeout_ms must be greater than zero");
    }

    let _ = &config.sessions_root;
    let _ = &config.state_file;

    Ok(())
}

fn print_help() {
    println!("codex-approval-watcher");
    println!();
    println!("Commands:");
    println!("  run [config-path]           Run the approval watcher");
    println!("  print-example-config        Print a sample config.toml");
    println!("  validate-config <path>      Validate a config file");
}

fn run(config: &Config) -> Result<()> {
    install_signal_handlers();
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
    let mut watcher = build_watcher(tx)?;
    watcher
        .watch(&config.sessions_root, RecursiveMode::Recursive)
        .with_context(|| {
            format!(
                "failed to watch sessions directory: {}",
                config.sessions_root.display()
            )
        })?;

    let mut state = load_state(&config.state_file);
    if !state.initialized {
        let existing_files = iter_session_files(&config.sessions_root)?;
        let bootstrap_meta = existing_files
            .iter()
            .filter_map(|path| {
                let signature = current_file_signature(path).ok()?;
                Some((path.display().to_string(), signature.0, signature.1))
            })
            .collect::<Vec<_>>();
        bootstrap_existing_files(&mut state, &bootstrap_meta);
        save_state(&config.state_file, &state)?;
    }

    while RUNNING.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(config.event_timeout_ms)) {
            Ok(Ok(event)) => {
                tick_event(config, &mut state, event)?;
            }
            Ok(Err(error)) => {
                eprintln!("[codex-approval-watcher] watch error: {error}");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("filesystem watcher channel disconnected");
            }
        }
    }

    drop(watcher);
    save_state(&config.state_file, &state)?;
    Ok(())
}

fn build_watcher(
    tx: std::sync::mpsc::Sender<notify::Result<Event>>,
) -> Result<RecommendedWatcher> {
    let watcher = notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })
    .context("failed to create filesystem watcher")?;
    Ok(watcher)
}

fn tick_event(config: &Config, state: &mut PersistedState, event: Event) -> Result<()> {
    if !should_process_event(&event.kind) {
        return Ok(());
    }

    let candidate_paths = expand_event_paths(&event.paths);
    if candidate_paths.is_empty() {
        return Ok(());
    }

    let live_paths = iter_session_files(&config.sessions_root)?
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<BTreeSet<_>>();

    for path in candidate_paths {
        if !path.is_file() {
            continue;
        }

        let key = path.display().to_string();
        let file_state = state.files.entry(key).or_insert_with(FileState::default);
        let events = process_file(&path, file_state)?;
        for event in events {
            if let Err(error) = notify_approval(&config.notifications, &event) {
                eprintln!("[codex-approval-watcher] {error}");
            }
            if let Err(error) = dispatch_event(&config.hooks, &event) {
                eprintln!("[codex-approval-watcher] {error}");
            }
        }
    }

    prune_deleted_files(state, &live_paths);
    save_state(&config.state_file, state)?;
    Ok(())
}

fn iter_session_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();

    paths.sort();
    Ok(paths)
}

fn should_process_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Remove(_)
            | EventKind::Any
            | EventKind::Other
    )
}

fn expand_event_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates = BTreeSet::new();

    for path in paths {
        if path.is_file() {
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                candidates.insert(path.clone());
            }
            continue;
        }

        if path.is_dir() {
            if let Ok(children) = iter_session_files(path) {
                for child in children {
                    candidates.insert(child);
                }
            }
        }
    }

    candidates.into_iter().collect()
}

#[cfg(unix)]
fn install_signal_handlers() {
    use std::ffi::c_int;

    unsafe extern "C" fn handle_signal(_signal: c_int) {
        RUNNING.store(false, Ordering::Relaxed);
    }

    unsafe extern "C" {
        fn signal(sig: c_int, handler: unsafe extern "C" fn(c_int)) -> usize;
    }

    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;

    unsafe {
        signal(SIGINT, handle_signal);
        signal(SIGTERM, handle_signal);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

#[cfg(test)]
mod tests {
    use super::{expand_event_paths, should_process_event, validate_config};
    use crate::config::{Config, HookConfig, NotificationsConfig};
    use notify::{EventKind, event::{CreateKind, ModifyKind}};
    use std::path::PathBuf;

    #[test]
    fn validate_config_accepts_notifications_without_hooks() {
        let config = Config {
            sessions_root: PathBuf::from("/tmp/sessions"),
            state_file: PathBuf::from("/tmp/state.json"),
            event_timeout_ms: 1000,
            notifications: NotificationsConfig {
                enabled: true,
                app: "Code".into(),
                sound: "Sosumi".into(),
            },
            hooks: Vec::new(),
        };

        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_config_requires_a_delivery_path() {
        let config = Config {
            sessions_root: PathBuf::from("/tmp/sessions"),
            state_file: PathBuf::from("/tmp/state.json"),
            event_timeout_ms: 1000,
            notifications: NotificationsConfig {
                enabled: false,
                app: "Code".into(),
                sound: "Sosumi".into(),
            },
            hooks: Vec::new(),
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_config_rejects_zero_hook_timeout() {
        let config = Config {
            sessions_root: PathBuf::from("/tmp/sessions"),
            state_file: PathBuf::from("/tmp/state.json"),
            event_timeout_ms: 1000,
            notifications: NotificationsConfig::default(),
            hooks: vec![HookConfig {
                name: "hook".into(),
                command: vec!["/tmp/hook".into()],
                timeout_ms: 0,
            }],
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn event_kind_filter_accepts_create_and_modify() {
        assert!(should_process_event(&EventKind::Create(CreateKind::File)));
        assert!(should_process_event(&EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any
        ))));
    }

    #[test]
    fn expand_event_paths_keeps_jsonl_files_only() {
        let temp = tempfile::tempdir().unwrap();
        let jsonl = temp.path().join("session.jsonl");
        let other = temp.path().join("note.txt");
        std::fs::write(&jsonl, "").unwrap();
        std::fs::write(&other, "").unwrap();

        let expanded = expand_event_paths(&[jsonl.clone(), other]);
        assert_eq!(expanded, vec![jsonl]);
    }
}
