# codex-approval-watcher

`codex-approval-watcher` is a small Rust service for macOS that watches Codex
session logs and emits a synthetic `approval.requested` event when it sees an
approval prompt. It exists for setups where approval requests are not surfaced
through the normal `notify` or `hooks` flow.

## What It Does

- Watches `~/.codex/sessions/**/*.jsonl`
- Uses `kqueue` notifications plus periodic reconciliation to avoid missed events
- Detects approval prompts from tool calls that request escalated permissions
- Sends a local macOS notification with `terminal-notifier` when available,
  otherwise falls back to `osascript`
- Forwards normalized `approval.requested` events to optional hooks

This project is intentionally focused on approval prompts. It does not try to
replace Codex's existing turn-complete notifications.

## Configuration

Start from [`config.example.toml`](./config.example.toml).

If you run the CLI without an explicit config path, it looks for:

```text
~/.config/codex-approval-watcher/config.toml
```

Current config fields:

- `sessions_root`: directory containing Codex session JSONL files
- `state_file`: persisted offset and metadata cache path
- `event_timeout_ms`: watcher receive timeout used for responsive shutdown
- `notifications`: built-in local notification settings
- `hooks`: optional commands that also receive `approval.requested` events

Each hook receives one JSON document on stdin:

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

[`config.homebrew.toml.example`](./config.homebrew.toml.example) shows a
service-friendly default layout for Homebrew installs.

[`examples/alfred-vscode-switcher.example.toml`](./examples/alfred-vscode-switcher.example.toml)
is an optional example for wiring approval events into an Alfred
`vscode-switcher` workflow setup.

## Usage

Validate the crate:

```sh
cargo check
```

Print the bundled example config:

```sh
cargo run -- print-example-config
```

Validate a config file:

```sh
cargo run -- validate-config ./config.toml
```

Run the watcher:

```sh
cargo run -- run
```

Or pass a config explicitly:

```sh
cargo run -- run ./config.toml
```

Send one test notification:

```sh
cargo run -- test-notification
```

Or pass a config explicitly:

```sh
cargo run -- test-notification ./config.toml
```

## Local Development

Run the test suite:

```sh
cargo test
```

Run the end-to-end self-test without `launchd`:

```sh
./self_test.sh
```

That script copies fixture session files into a temporary Codex home, starts
the watcher as a child process, appends a synthetic approval line, and verifies
that the configured hook receives an `approval.requested` event.

## Dev Helper

[`dev/install_service.sh`](./dev/install_service.sh) installs a repo-local
`launchd` agent for development and personal use.

By default it expects `./config.toml` in the repository root. You can also pass
a config path explicitly:

```sh
cp config.example.toml config.toml
./dev/install_service.sh install
./dev/install_service.sh restart ./examples/alfred-vscode-switcher.example.toml
```

Supported commands:

- `install`
- `restart`
- `uninstall`
- `status`
- `build`

## Homebrew

A draft formula lives at
[`homebrew/codex-approval-watcher.rb`](./homebrew/codex-approval-watcher.rb).

Typical release flow:

1. Push this repository to GitHub.
2. Create a tag such as `v0.1.0`.
3. Build the release tarball and compute its `sha256`.
4. Replace the placeholder `homepage`, `url`, and `sha256` in the formula.
5. Copy the formula into a personal tap such as `your-user/homebrew-tap`.
6. Install with `brew install your-user/tap/codex-approval-watcher`.
7. Start the service with `brew services start codex-approval-watcher`.

The formula installs only `config.homebrew.toml.example` and copies it into:

```text
~/.config/codex-approval-watcher/config.toml
```

on first install, so the service and the CLI share the same default config
location.
