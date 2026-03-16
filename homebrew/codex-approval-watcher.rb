class CodexApprovalWatcher < Formula
  desc "Watch Codex session logs and emit approval.requested events"
  homepage "https://github.com/YOUR_GITHUB_USER/codex-approval-watcher"
  url "https://github.com/YOUR_GITHUB_USER/codex-approval-watcher/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_SHA256"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
    pkgshare.install "config.example.toml"
    pkgshare.install "config.homebrew.toml.example"
  end

  def post_install
    config_path = etc/"codex-approval-watcher.toml"
    return if config_path.exist?

    cp pkgshare/"config.homebrew.toml.example", config_path
  end

  service do
    run [opt_bin/"codex-approval-watcher", "run", etc/"codex-approval-watcher.toml"]
    keep_alive true
    log_path var/"log/codex-approval-watcher.log"
    error_log_path var/"log/codex-approval-watcher.log"
  end

  test do
    assert_match "codex-approval-watcher", shell_output("#{bin}/codex-approval-watcher --help")
  end
end
