class CodexApprovalWatcher < Formula
  desc "Watch Codex session logs and emit approval.requested events"
  homepage "https://github.com/danbi2990/codex-approval-watcher"
  url "https://github.com/danbi2990/codex-approval-watcher/archive/refs/tags/v0.1.2.tar.gz"
  sha256 "fb6795b787780dafd3b953355824e169e219d2df34c885047b2c73eaf98b4492"
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
      The default config file will be created automatically on first run at:

        ~/.config/codex-approval-watcher/config.toml

      Edit it after the first run if you want to customize hooks or other settings.

      You can preview the default template with:

        cat #{opt_pkgshare}/config.homebrew.toml.example
    EOS
  end

  test do
    assert_match "codex-approval-watcher", shell_output("#{bin}/codex-approval-watcher --help")
  end
end
