# codex-approval-watcher

`codex-approval-watcher` is a small Rust service for macOS that watches Codex
session logs and emits a synthetic `approval.requested` event when it sees an
approval prompt. It exists for setups where approval requests are not surfaced
through the normal `notify` or `hooks` flow.

## What It Does

- Watches `~/.codex/sessions/**/*.jsonl`
- Uses `kqueue` notifications plus periodic reconciliation to avoid missed events
- Detects approval prompts from tool calls that request escalated permissions
- Sends a local macOS notification with `osascript`
- Forwards normalized `approval.requested` events to optional hooks

This project is intentionally focused on approval prompts. It does not try to
replace Codex's existing turn-complete notifications.

## Homebrew

Install from the published tap:

```sh
brew tap danbi2990/tap
brew install codex-approval-watcher
```

Start the background service:

```sh
brew services start codex-approval-watcher
```

Verify notifications after install:

```sh
/opt/homebrew/bin/codex-approval-watcher doctor-notifications
```

On first run the watcher creates:

```text
~/.config/codex-approval-watcher/config.toml
```

automatically. If you want to inspect the template first, see:

```text
/opt/homebrew/opt/codex-approval-watcher/share/codex-approval-watcher/config.homebrew.toml.example
```

## Configuration

If you run the CLI without an explicit config path, it uses:

```text
~/.config/codex-approval-watcher/config.toml
```

Use [`config.example.toml`](./config.example.toml) for local/manual runs, or
inspect [`config.homebrew.toml.example`](./config.homebrew.toml.example) for the
Homebrew-oriented default layout.

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

Send a notification and verify delivery markers from macOS unified logs:

```sh
cargo run -- doctor-notifications
```

## Local Development

Verification gate for changes in this repository:

```sh
cargo test
cargo clippy --all-targets --all-features -- -W clippy::pedantic
```

`cargo clippy` should be run with `clippy::pedantic` enabled for local
verification before shipping changes.

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

## Homebrew Packaging

The tap formula is based on:
[`homebrew/codex-approval-watcher.rb`](./homebrew/codex-approval-watcher.rb).

Typical maintainer release flow:

1. Push this repository to GitHub.
2. Create a tag such as `v0.1.0`.
3. Build the release tarball and compute its `sha256`.
4. Replace the placeholder `homepage`, `url`, and `sha256` in the formula.
5. Copy the formula into a personal tap such as `your-user/homebrew-tap`.
6. Install with `brew install your-user/tap/codex-approval-watcher`.
7. Start the service with `brew services start codex-approval-watcher`.
