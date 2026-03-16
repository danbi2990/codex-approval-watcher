#!/bin/zsh

set -euo pipefail

repo_dir="/Users/jake/Downloads/Development/alfred-workflow/codex-approval-watcher"
plist_template="$repo_dir/launchd/com.jake.codex-approval-watcher.plist"
launch_agents_dir="$HOME/Library/LaunchAgents"
installed_plist="$launch_agents_dir/com.jake.codex-approval-watcher.plist"
binary_path="$repo_dir/target/release/codex-approval-watcher"
config_path="$repo_dir/config.vscode-switcher.toml"
manifest_path="$repo_dir/Cargo.toml"

usage() {
  cat <<EOF
Usage: ./install_service.sh [install|uninstall|restart|status|build]

install   Build the release binary, install the launchd service, and start it
build     Build the release binary only
uninstall Remove and stop the launchd watcher service
restart   Rebuild and reload the launchd watcher service
status    Show whether the launchd watcher service is loaded
EOF
}

render_plist() {
  /usr/bin/sed \
    -e "s|__BINARY_PATH__|$binary_path|g" \
    -e "s|__CONFIG_PATH__|$config_path|g" \
    "$plist_template"
}

build_binary() {
  cargo build --release --offline --manifest-path "$manifest_path"
}

install_service() {
  build_binary
  mkdir -p "$launch_agents_dir"
  render_plist > "$installed_plist"
  launchctl bootout "gui/$(id -u)" "$installed_plist" >/dev/null 2>&1 || true
  launchctl bootstrap "gui/$(id -u)" "$installed_plist"
  launchctl kickstart -k "gui/$(id -u)/com.jake.codex-approval-watcher"
  echo "Installed watcher: $installed_plist"
}

uninstall_service() {
  launchctl bootout "gui/$(id -u)" "$installed_plist" >/dev/null 2>&1 || true
  rm -f "$installed_plist"
  echo "Removed watcher: $installed_plist"
}

status_service() {
  if launchctl print "gui/$(id -u)/com.jake.codex-approval-watcher" >/dev/null 2>&1; then
    echo "loaded"
  else
    echo "not loaded"
  fi
}

action="${1:-install}"

case "$action" in
  install)
    install_service
    ;;
  build)
    build_binary
    ;;
  uninstall)
    uninstall_service
    ;;
  restart)
    install_service
    ;;
  status)
    status_service
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
