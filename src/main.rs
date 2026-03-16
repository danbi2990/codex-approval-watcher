mod config;
mod dispatcher;
mod models;
mod notifier;
mod session_parser;
mod state;

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    env, fs,
    os::fd::IntoRawFd,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use config::{Config, expand_config_paths};
use dispatcher::dispatch_event;
use libc::{self, c_int};
use models::{ApprovalEvent, PersistedState};
use notifier::notify_approval;
use serde::Deserialize;
use session_parser::{current_file_signature, hydrate_session_metadata, process_file};
use state::{bootstrap_existing_files, load_state, prune_deleted_files, save_state};
use walkdir::WalkDir;

const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");
static RUNNING: AtomicBool = AtomicBool::new(true);
const TOPOLOGY_RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const MAX_WATCHED_SESSION_FILES: usize = 16;
const MAX_RECENT_SESSION_IDS: usize = 64;

#[derive(Debug, Default)]
struct SessionTree {
    files: Vec<SessionFile>,
}

#[derive(Debug, Clone)]
struct SessionFile {
    path: PathBuf,
    session_id: Option<String>,
    mtime_ns: u64,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct SessionIndexEntry {
    id: String,
}

struct VnodeWatcher {
    queue: c_int,
    paths_by_fd: BTreeMap<c_int, String>,
}

impl VnodeWatcher {
    fn new() -> Result<Self> {
        let queue = unsafe { libc::kqueue() };
        if queue == -1 {
            bail!(
                "failed to create kqueue watcher: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok(Self {
            queue,
            paths_by_fd: BTreeMap::new(),
        })
    }

    fn rebuild(&mut self, paths: &BTreeSet<String>) -> Result<()> {
        let mut next = Self::new()?;

        for path in paths {
            if let Err(error) = next.add_path(path) {
                eprintln!("[codex-approval-watcher] failed to add file watch for {path}: {error}");
            }
        }

        *self = next;
        Ok(())
    }

    fn add_path(&mut self, path: &str) -> Result<()> {
        let file =
            fs::File::open(path).with_context(|| format!("failed to open watched file: {path}"))?;
        let fd = file.into_raw_fd();

        let event = libc::kevent {
            ident: fd as _,
            filter: libc::EVFILT_VNODE,
            flags: (libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR) as _,
            fflags: vnode_watch_flags(),
            data: 0,
            udata: std::ptr::null_mut(),
        };

        let ret = unsafe {
            libc::kevent(
                self.queue,
                &event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };

        if ret == -1 {
            let error = std::io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(error).with_context(|| format!("failed to register kqueue watch: {path}"));
        }

        self.paths_by_fd.insert(fd, path.to_string());
        Ok(())
    }

    fn poll_path(&self, timeout: Duration) -> Result<Option<PathBuf>> {
        let mut event = libc::kevent {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let timeout = libc::timespec {
            tv_sec: timeout.as_secs() as _,
            tv_nsec: timeout.subsec_nanos() as _,
        };

        let ret = unsafe { libc::kevent(self.queue, std::ptr::null(), 0, &mut event, 1, &timeout) };

        match ret {
            -1 => {
                let error = std::io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(code) if code == libc::EINTR) {
                    Ok(None)
                } else {
                    Err(error).context("failed to poll kqueue watcher")
                }
            }
            0 => Ok(None),
            _ => {
                if event.flags & libc::EV_ERROR != 0 {
                    let errno = event.data as i32;
                    if errno != 0 {
                        return Err(std::io::Error::from_raw_os_error(errno))
                            .context("kqueue delivered an error event");
                    }
                }

                let fd = event.ident as c_int;
                Ok(self.paths_by_fd.get(&fd).map(PathBuf::from))
            }
        }
    }
}

impl Drop for VnodeWatcher {
    fn drop(&mut self) {
        for fd in self.paths_by_fd.keys().copied() {
            unsafe {
                libc::close(fd);
            }
        }
        unsafe {
            libc::close(self.queue);
        }
    }
}

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
        Some("test-notification") => {
            let config_path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("config.toml"));
            let config = load_config(&config_path)?;
            validate_config(&config)?;
            test_notification(&config)
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
    println!("  test-notification [config]  Send one local approval notification");
    println!("  print-example-config        Print a sample config.toml");
    println!("  validate-config <path>      Validate a config file");
}

fn run(config: &Config) -> Result<()> {
    install_signal_handlers();
    let mut watcher = VnodeWatcher::new()?;
    let mut watched_files = BTreeSet::new();
    let mut last_topology_reconcile = std::time::Instant::now();
    let (delivery_tx, delivery_thread) = spawn_delivery_worker(config.clone());
    let session_index_path = session_index_path(&config.sessions_root);

    let mut state = load_state(&config.state_file);
    let mut state_dirty = false;
    let initial_tree = scan_session_tree(&config.sessions_root)?;
    let recent_session_ids = load_recent_session_ids(&session_index_path)?;
    let bootstrapping = !state.initialized;
    if bootstrapping {
        let bootstrap_meta = initial_tree
            .files
            .iter()
            .map(|file| {
                (
                    file.path.display().to_string(),
                    file.mtime_ns,
                    file.size,
                )
            })
            .collect::<Vec<_>>();
        bootstrap_existing_files(&mut state, &bootstrap_meta);
        state_dirty = true;
    }

    state_dirty |= hydrate_missing_metadata(&initial_tree.files, &mut state)?;
    let initial_added_watch_paths = sync_watches(
        &initial_tree,
        &recent_session_ids,
        &mut watcher,
        &mut watched_files,
    )?;
    let initial_scan_paths = if bootstrapping {
        initial_tree
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
    } else {
        reconcile_scan_paths(&initial_tree, &state, &initial_added_watch_paths)
    };
    let (files_dirty, initial_events) = sync_session_files(&initial_scan_paths, &mut state)?;
    state_dirty |= files_dirty;
    if state_dirty {
        save_state(&config.state_file, &state)?;
        state_dirty = false;
    }
    dispatch_events(&delivery_tx, initial_events);

    while RUNNING.load(Ordering::Relaxed) {
        let mut pending_events = Vec::new();
        match watcher.poll_path(Duration::from_millis(config.event_timeout_ms)) {
            Ok(Some(path)) => {
                let (files_dirty, events) = sync_session_files(&[path], &mut state)?;
                state_dirty |= files_dirty;
                pending_events.extend(events);
            }
            Ok(None) => {
                if last_topology_reconcile.elapsed() >= TOPOLOGY_RESCAN_INTERVAL {
                    let tree = scan_session_tree(&config.sessions_root)?;
                    let recent_session_ids = load_recent_session_ids(&session_index_path)?;
                    let added_watch_paths =
                        sync_watches(&tree, &recent_session_ids, &mut watcher, &mut watched_files)?;
                    state_dirty |= hydrate_missing_metadata(&tree.files, &mut state)?;
                    let reconcile_paths = reconcile_scan_paths(&tree, &state, &added_watch_paths);
                    let (files_dirty, events) = sync_session_files(&reconcile_paths, &mut state)?;
                    state_dirty |= files_dirty;
                    pending_events.extend(events);
                    let live_paths = tree
                        .files
                        .iter()
                        .map(|file| file.path.display().to_string())
                        .collect::<BTreeSet<_>>();
                    state_dirty |= prune_deleted_files(&mut state, &live_paths);
                    last_topology_reconcile = std::time::Instant::now();
                }
            }
            Err(error) => {
                eprintln!("[codex-approval-watcher] watch error: {error}");
            }
        }

        if state_dirty {
            save_state(&config.state_file, &state)?;
            state_dirty = false;
        }
        dispatch_events(&delivery_tx, pending_events);
    }

    if state_dirty {
        save_state(&config.state_file, &state)?;
    }
    drop(delivery_tx);
    let _ = delivery_thread.join();
    Ok(())
}

fn hydrate_missing_metadata(files: &[SessionFile], state: &mut PersistedState) -> Result<bool> {
    let live_paths = files
        .iter()
        .map(|file| file.path.display().to_string())
        .collect::<BTreeSet<_>>();
    let mut dirty = false;

    for path in &live_paths {
        let file_state = state.files.entry(path.clone()).or_default();
        let before = file_state.clone();

        let metadata_missing = file_state.cwd.as_deref().unwrap_or("").is_empty()
            || file_state.session_id.as_deref().unwrap_or("").is_empty();
        if !metadata_missing {
            continue;
        }

        hydrate_session_metadata(Path::new(path), file_state)?;
        dirty |= *file_state != before;
    }

    Ok(prune_deleted_files(state, &live_paths) || dirty)
}

fn sync_watches(
    tree: &SessionTree,
    recent_session_ids: &[String],
    watcher: &mut VnodeWatcher,
    watched_files: &mut BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let desired_files = select_watched_files(tree, recent_session_ids);
    if desired_files == *watched_files {
        return Ok(BTreeSet::new());
    }
    let added_watch_paths = desired_files
        .difference(watched_files)
        .cloned()
        .collect::<BTreeSet<_>>();
    watcher.rebuild(&desired_files)?;
    *watched_files = desired_files;
    Ok(added_watch_paths)
}

fn sync_session_files(
    files: &[PathBuf],
    state: &mut PersistedState,
) -> Result<(bool, Vec<ApprovalEvent>)> {
    let mut dirty = false;
    let mut events = Vec::new();

    for path in files {
        let key = path.display().to_string();
        let file_state = state.files.entry(key).or_default();
        let before = file_state.clone();
        let file_events = process_file(path, file_state)?;
        dirty |= *file_state != before;
        events.extend(file_events);
    }

    Ok((dirty, events))
}

fn scan_session_tree(root: &Path) -> Result<SessionTree> {
    if !root.is_dir() {
        return Ok(SessionTree::default());
    }

    let mut tree = SessionTree::default();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        let path = entry.path();
        if entry.file_type().is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        {
            let (mtime_ns, size) = current_file_signature(path)?;
            tree.files.push(SessionFile {
                path: path.to_path_buf(),
                session_id: session_id_from_rollout_path(path),
                mtime_ns,
                size,
            });
        }
    }
    tree.files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(tree)
}

fn session_index_path(sessions_root: &Path) -> PathBuf {
    sessions_root
        .parent()
        .map(|parent| parent.join("session_index.jsonl"))
        .unwrap_or_else(|| PathBuf::from("session_index.jsonl"))
}

fn load_recent_session_ids(path: &Path) -> Result<Vec<String>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read session index: {}", path.display()));
        }
    };

    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    for line in raw.lines().rev() {
        let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(line) else {
            continue;
        };
        if seen.insert(entry.id.clone()) {
            ids.push(entry.id);
            if ids.len() >= MAX_RECENT_SESSION_IDS {
                break;
            }
        }
    }

    Ok(ids)
}

fn select_watched_files(tree: &SessionTree, recent_session_ids: &[String]) -> BTreeSet<String> {
    let mut selected = Vec::new();

    for session_id in recent_session_ids {
        if selected.len() >= MAX_WATCHED_SESSION_FILES {
            break;
        }
        if let Some(file) = tree
            .files
            .iter()
            .find(|file| file.session_id.as_deref() == Some(session_id.as_str()))
        {
            let path = file.path.display().to_string();
            if !selected.iter().any(|existing| existing == &path) {
                selected.push(path);
            }
        }
    }

    let mut fallback = tree
        .files
        .iter()
        .map(|file| (Reverse(file.mtime_ns), file.path.display().to_string()))
        .collect::<Vec<_>>();
    fallback.sort_unstable();

    for (_, path) in fallback {
        if selected.len() >= MAX_WATCHED_SESSION_FILES {
            break;
        }
        if !selected.iter().any(|existing| existing == &path) {
            selected.push(path);
        }
    }

    selected.into_iter().collect()
}

fn reconcile_scan_paths(
    tree: &SessionTree,
    state: &PersistedState,
    added_watch_paths: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();

    for file in &tree.files {
        let key = file.path.display().to_string();
        if added_watch_paths.contains(&key) {
            paths.insert(file.path.clone());
            continue;
        }

        match state.files.get(&key) {
            Some(file_state)
                if file_state.mtime_ns == file.mtime_ns && file_state.size == file.size => {}
            _ => {
                paths.insert(file.path.clone());
            }
        }
    }

    paths.into_iter().collect()
}

fn session_id_from_rollout_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.len() < 36 {
        return None;
    }

    let candidate = &stem[stem.len() - 36..];
    if looks_like_session_id(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn looks_like_session_id(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, ch)| match index {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

fn vnode_watch_flags() -> u32 {
    libc::NOTE_WRITE
        | libc::NOTE_EXTEND
        | libc::NOTE_ATTRIB
        | libc::NOTE_RENAME
        | libc::NOTE_DELETE
        | libc::NOTE_REVOKE
}

fn test_notification(config: &Config) -> Result<()> {
    let event = crate::models::ApprovalEvent {
        message: "Test approval notification from codex-approval-watcher".into(),
        ..build_test_event(
            env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            "echo test",
        )
    };

    notify_approval(&config.notifications, &event)
}

fn build_test_event(cwd: PathBuf, command: &str) -> crate::models::ApprovalEvent {
    crate::models::ApprovalEvent {
        event: "approval.requested",
        session_id: "test-session".into(),
        cwd: cwd.display().to_string(),
        timestamp: "1970-01-01T00:00:00Z".into(),
        message: "Test approval request from codex-approval-watcher".into(),
        command: command.into(),
    }
}

fn deliver_event(config: &Config, event: &crate::models::ApprovalEvent) {
    if let Err(error) = notify_approval(&config.notifications, event) {
        eprintln!("[codex-approval-watcher] {error}");
    }
    if let Err(error) = dispatch_event(&config.hooks, event) {
        eprintln!("[codex-approval-watcher] {error}");
    }
}

fn spawn_delivery_worker(config: Config) -> (mpsc::Sender<ApprovalEvent>, thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<ApprovalEvent>();
    let handle = thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            deliver_event(&config, &event);
        }
    });
    (tx, handle)
}

fn dispatch_events(sender: &mpsc::Sender<ApprovalEvent>, events: Vec<ApprovalEvent>) {
    for event in events {
        if sender.send(event).is_err() {
            break;
        }
    }
}

#[cfg(unix)]
fn install_signal_handlers() {
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
    use super::{
        SessionFile, SessionTree, build_test_event, hydrate_missing_metadata,
        load_recent_session_ids, reconcile_scan_paths, scan_session_tree,
        select_watched_files, session_id_from_rollout_path, validate_config, vnode_watch_flags,
    };
    use crate::config::{Config, HookConfig, NotificationsConfig};
    use crate::models::{FileState, PersistedState};
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::PathBuf,
    };

    #[test]
    fn validate_config_accepts_notifications_without_hooks() {
        let config = Config {
            sessions_root: PathBuf::from("/tmp/sessions"),
            state_file: PathBuf::from("/tmp/state.json"),
            event_timeout_ms: 1000,
            notifications: NotificationsConfig {
                enabled: true,
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
    fn scan_session_tree_includes_nested_directories() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("2026/03/16");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("session.jsonl");
        std::fs::write(&file, "").unwrap();

        let tree = scan_session_tree(temp.path()).unwrap();
        assert!(tree.files.iter().any(|entry| entry.path == file));
    }

    #[test]
    fn file_watch_flags_include_write() {
        let flags = vnode_watch_flags();
        assert_ne!(flags & libc::NOTE_WRITE, 0);
        assert_ne!(flags & libc::NOTE_DELETE, 0);
    }

    #[test]
    fn hydrate_missing_metadata_recovers_bootstrapped_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let session_path = temp.path().join("session.jsonl");
        std::fs::write(
            &session_path,
            concat!(
                "{\"timestamp\":\"2026-03-16T00:00:00Z\",\"type\":\"session_meta\",",
                "\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp/project\"}}\n"
            ),
        )
        .unwrap();

        let mut state = PersistedState {
            initialized: true,
            files: BTreeMap::from([(
                session_path.display().to_string(),
                crate::models::FileState {
                    offset: std::fs::metadata(&session_path).unwrap().len(),
                    cwd: None,
                    mtime_ns: 0,
                    session_id: None,
                    seen_calls: Vec::new(),
                    size: std::fs::metadata(&session_path).unwrap().len(),
                },
            )]),
        };

        hydrate_missing_metadata(
            &[SessionFile {
                path: session_path.clone(),
                session_id: Some("sess-1".into()),
                mtime_ns: 0,
                size: std::fs::metadata(&session_path).unwrap().len(),
            }],
            &mut state,
        )
        .unwrap();

        let recovered = &state.files[&session_path.display().to_string()];
        assert_eq!(recovered.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(recovered.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn session_id_is_extracted_from_rollout_filename() {
        let path = PathBuf::from(
            "/tmp/rollout-2026-03-16T17-35-37-019cf5c9-5063-7450-86bd-6af537dea0bf.jsonl",
        );

        assert_eq!(
            session_id_from_rollout_path(&path).as_deref(),
            Some("019cf5c9-5063-7450-86bd-6af537dea0bf")
        );
    }

    #[test]
    fn recent_session_ids_prefer_latest_unique_entries() {
        let temp = tempfile::tempdir().unwrap();
        let index_path = temp.path().join("session_index.jsonl");
        fs::write(
            &index_path,
            concat!(
                "{\"id\":\"sess-old\",\"thread_name\":\"old\",\"updated_at\":\"2026-03-16T00:00:00Z\"}\n",
                "{\"id\":\"sess-new\",\"thread_name\":\"new\",\"updated_at\":\"2026-03-16T00:01:00Z\"}\n",
                "{\"id\":\"sess-old\",\"thread_name\":\"old\",\"updated_at\":\"2026-03-16T00:02:00Z\"}\n"
            ),
        )
        .unwrap();

        let ids = load_recent_session_ids(&index_path).unwrap();
        assert_eq!(ids, vec!["sess-old".to_string(), "sess-new".to_string()]);
    }

    #[test]
    fn watched_files_prioritize_recent_session_ids() {
        let tree = SessionTree {
            files: vec![
                SessionFile {
                    path: PathBuf::from("/tmp/a.jsonl"),
                    session_id: Some("sess-a".into()),
                    mtime_ns: 10,
                    size: 10,
                },
                SessionFile {
                    path: PathBuf::from("/tmp/b.jsonl"),
                    session_id: Some("sess-b".into()),
                    mtime_ns: 30,
                    size: 10,
                },
                SessionFile {
                    path: PathBuf::from("/tmp/c.jsonl"),
                    session_id: Some("sess-c".into()),
                    mtime_ns: 20,
                    size: 10,
                },
            ],
        };

        let watched = select_watched_files(&tree, &[String::from("sess-a")]);
        assert!(watched.contains("/tmp/a.jsonl"));
        assert!(watched.contains("/tmp/b.jsonl"));
        assert!(watched.contains("/tmp/c.jsonl"));
    }

    #[test]
    fn reconcile_scan_paths_include_newly_added_watch_files() {
        let tree = SessionTree {
            files: vec![SessionFile {
                path: PathBuf::from("/tmp/new.jsonl"),
                session_id: Some("sess-new".into()),
                mtime_ns: 11,
                size: 42,
            }],
        };
        let state = PersistedState {
            initialized: true,
            files: BTreeMap::new(),
        };
        let added_watch_paths = BTreeSet::from(["/tmp/new.jsonl".to_string()]);

        let paths = reconcile_scan_paths(&tree, &state, &added_watch_paths);
        assert_eq!(paths, vec![PathBuf::from("/tmp/new.jsonl")]);
    }

    #[test]
    fn reconcile_scan_paths_include_files_with_changed_signature_even_if_not_watched() {
        let tree = SessionTree {
            files: vec![SessionFile {
                path: PathBuf::from("/tmp/cold.jsonl"),
                session_id: Some("sess-cold".into()),
                mtime_ns: 22,
                size: 200,
            }],
        };
        let state = PersistedState {
            initialized: true,
            files: BTreeMap::from([(
                "/tmp/cold.jsonl".into(),
                FileState {
                    offset: 100,
                    cwd: Some("/tmp/project".into()),
                    mtime_ns: 10,
                    session_id: Some("sess-cold".into()),
                    seen_calls: Vec::new(),
                    size: 100,
                },
            )]),
        };

        let paths = reconcile_scan_paths(&tree, &state, &BTreeSet::new());
        assert_eq!(paths, vec![PathBuf::from("/tmp/cold.jsonl")]);
    }

    #[test]
    fn build_test_event_uses_requested_cwd() {
        let event = build_test_event(PathBuf::from("/tmp/project-x"), "echo ok");

        assert_eq!(event.event, "approval.requested");
        assert_eq!(event.cwd, "/tmp/project-x");
        assert_eq!(event.command, "echo ok");
        assert_eq!(event.session_id, "test-session");
    }
}
