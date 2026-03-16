mod config;
mod dispatcher;
mod models;
mod session_parser;
mod state;

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use config::{Config, expand_config_paths};
use dispatcher::dispatch_event;
use models::{FileState, PersistedState};
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
    if config.hooks.is_empty() {
        bail!("config must contain at least one hook");
    }

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

    if config.poll_interval_ms == 0 {
        bail!("poll_interval_ms must be greater than zero");
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
        tick(config, &mut state)?;
        thread::sleep(Duration::from_millis(config.poll_interval_ms));
    }

    save_state(&config.state_file, &state)?;
    Ok(())
}

fn tick(config: &Config, state: &mut PersistedState) -> Result<()> {
    let session_files = iter_session_files(&config.sessions_root)?;
    let live_paths = session_files
        .iter()
        .map(|path| path.display().to_string())
        .collect::<BTreeSet<_>>();

    for path in &session_files {
        let key = path.display().to_string();
        let file_state = state.files.entry(key).or_insert_with(FileState::default);
        let events = process_file(path, file_state)?;
        for event in events {
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
