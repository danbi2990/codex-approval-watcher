class CodexApprovalWatcher < Formula
  desc "Watch Codex session logs and emit approval.requested events"
  homepage "https://github.com/danbi2990/codex-approval-watcher"
  url "https://github.com/danbi2990/codex-approval-watcher/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "f7f8924ff84388425c777ec6cec04af8d6cd3281a26f1dffb11a9bc743595ec1"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
    pkgshare.install "config.homebrew.toml.example"
  end

  def post_install
    config_dir = Pathname.new(File.expand_path("~/.config/codex-approval-watcher"))
    config_dir.mkpath
    config_path = config_dir/"config.toml"
    return if config_path.exist?

    cp pkgshare/"config.homebrew.toml.example", config_path
  end

  service do
    run [opt_bin/"codex-approval-watcher", "run"]
    keep_alive true
    log_path var/"log/codex-approval-watcher.log"
    error_log_path var/"log/codex-approval-watcher.log"
  end

  test do
    assert_match "codex-approval-watcher", shell_output("#{bin}/codex-approval-watcher --help")
  end
end
