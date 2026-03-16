#!/bin/zsh

set -euo pipefail

repo_dir="${0:A:h}"
fixture_dir="$repo_dir/fixtures/self-test"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-approval-selftest.XXXXXX")"
binary_path="$repo_dir/target/debug/codex-approval-watcher"
config_path="$work_dir/config.toml"
capture_path="$work_dir/output/event.json"
log_path="$work_dir/watcher.log"
session_id="019cf5c9-5063-7450-86bd-6af537dea0bf"
session_file="$work_dir/codex-home/sessions/2026/03/16/rollout-2026-03-16T00-00-00-$session_id.jsonl"

cleanup() {
  if [[ -n "${watcher_pid:-}" ]]; then
    kill "$watcher_pid" >/dev/null 2>&1 || true
    wait "$watcher_pid" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

mkdir -p "$work_dir"
cp -R "$fixture_dir/codex-home" "$work_dir/codex-home"

cat > "$config_path" <<EOF
sessions_root = "$work_dir/codex-home/sessions"
state_file = "$work_dir/state/state.json"
event_timeout_ms = 200

[notifications]
enabled = false
sound = "Sosumi"

[[hooks]]
name = "capture"
command = ["$fixture_dir/capture_event.sh", "$capture_path"]
timeout_ms = 1000
EOF

cargo build --manifest-path "$repo_dir/Cargo.toml" >/dev/null

"$binary_path" run "$config_path" >"$log_path" 2>&1 &
watcher_pid=$!

sleep 1

cat >> "$session_file" <<'EOF'
{"timestamp":"2026-03-16T00:00:01Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"printf self-test > /tmp/self-test\",\"justification\":\"Self test approval\",\"sandbox_permissions\":\"require_escalated\"}","call_id":"self-test-call-1"}}
EOF

for _ in {1..40}; do
  if [[ -f "$capture_path" ]] && grep -q '"event":"approval.requested"' "$capture_path"; then
    echo "self-test passed"
    echo "work_dir=$work_dir"
    echo "capture_path=$capture_path"
    exit 0
  fi
  sleep 0.25
done

echo "self-test failed"
echo "work_dir=$work_dir"
echo "--- watcher.log ---"
cat "$log_path" || true
echo "--- capture ---"
cat "$capture_path" || true
exit 1
