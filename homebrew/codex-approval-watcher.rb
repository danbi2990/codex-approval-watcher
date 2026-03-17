class CodexApprovalWatcher < Formula
  desc "Watch Codex session logs and emit approval.requested events"
  homepage "https://github.com/danbi2990/codex-approval-watcher"
  url "https://github.com/danbi2990/codex-approval-watcher/archive/refs/tags/v0.1.1.tar.gz"
  sha256 "ca2e22a556cab3c30fcb6d6dd1cf08770dc63a1ad94711d33ebd47288875062a"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
    pkgshare.install "config.homebrew.toml.example"
  end

  service do
    run [opt_bin/"codex-approval-watcher", "run"]
    keep_alive true
    log_path var/"log/codex-approval-watcher.log"
    error_log_path var/"log/codex-approval-watcher.log"
  end

  def caveats
    <<~EOS
      Create a config file before starting the service:

        mkdir -p ~/.config/codex-approval-watcher
        cp #{opt_pkgshare}/config.homebrew.toml.example ~/.config/codex-approval-watcher/config.toml

      Then edit ~/.config/codex-approval-watcher/config.toml to fit your setup.
    EOS
  end

  test do
    assert_match "codex-approval-watcher", shell_output("#{bin}/codex-approval-watcher --help")
  end
end
