# codex-approval-watcher

`codex-approval-watcher` is a small Rust service that fills a specific gap in
Codex: approval requests are not currently surfaced through the built-in
`notify`/`hooks` flow in this environment, so the watcher reads Codex session
logs and emits a synthetic `approval.requested` event to registered hooks.

This project lives as a sibling of `vscode-switcher` inside the current Git
repo so both can evolve together for now. It is intentionally structured as an
independent Rust crate so it can be extracted into its own public repository
later without much churn.

## Scope

- Watches `~/.codex/sessions/**/*.jsonl`
- Uses `kqueue` directory notifications on macOS and rescans changed session files
- Detects approval requests by looking for function calls with
  `sandbox_permissions = "require_escalated"`
- Sends a local macOS notification via `terminal-notifier` when available,
  otherwise falls back to `osascript`
- Emits a normalized `approval.requested` event to configured hooks
- Leaves all consumer-specific behavior to hooks

`codex-approval-watcher` is intentionally approval-specific. It does not try to
replace Codex's existing `turn completed` notifications.

## Configuration

See [config.example.toml](/Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/config.example.toml) for the expected shape.
For the local `vscode-switcher` integration in this repo, see
[config.vscode-switcher.toml](/Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/config.vscode-switcher.toml).

Current config model:

- `sessions_root`: directory containing Codex session JSONL files
- `state_file`: local offset/metadata cache path
- `event_timeout_ms`: watcher receive timeout used for a responsive shutdown loop
- `notifications`: built-in local notification delivery settings
- `hooks`: optional commands that should also receive `approval.requested` events

Each hook receives one JSON document on stdin with this shape:

```json
{
  "event": "approval.requested",
  "session_id": "019cf0bd-b079-7b32-b46b-c398698ff9c6",
  "cwd": "/path/to/project",
  "timestamp": "2026-03-16T00:00:00Z",
  "message": "Do you want to allow writing outside the workspace?",
  "command": "printf 'hi' > /tmp/example.txt"
}
```

## Development

Validate the crate:

```sh
cargo check --manifest-path /Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/Cargo.toml
```

Print the bundled example config:

```sh
cargo run --manifest-path /Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/Cargo.toml -- print-example-config
```

Validate a config file:

```sh
cargo run --manifest-path /Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/Cargo.toml -- validate-config ./config.example.toml
```

Run the watcher:

```sh
cargo run --manifest-path /Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/Cargo.toml -- run ./config.example.toml
```

Run the local self-test loop without `launchd` or a real approval prompt:

```sh
zsh /Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher/self_test.sh
```

That script copies fixture session files into a temporary `codex-home`, starts
the watcher as a child process, appends a synthetic approval line to the
fixture rollout JSONL, and verifies that the configured hook receives an
`approval.requested` event.

Build and install the local `launchd` service for this repo:

```sh
./install_service.sh
```

## Extraction Later

If this should become a standalone public repository later, split it out with a
history-preserving command like:

```sh
git subtree split --prefix=codex-approval-watcher -b split-codex-approval-watcher
```
