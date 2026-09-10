//! Actual-process coverage for Pane's discoverable top-level entry point.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "pane-entrypoint-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pane() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pane"));
    command
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        // **Set, so these tests stay about the entry point.** A session with
        // no base URL starts an `inference-gateway` and refuses when it
        // cannot (`gateway::start_or_attach`); a set URL is the attach half,
        // and needs no binary. Port 1 is never dialled -- no input here is a
        // turn -- and could only refuse locally if it were.
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
        // Lifecycle reporting degrades when Glasshouse is absent. Keeping it
        // absent makes these tests independent of the developer's install --
        // and proves a session needs no `glasshouse` on `PATH` at all.
        .env("PATH", "");
    command
}

#[test]
fn top_level_help_is_real_usage_on_stdout() {
    for flag in ["--help", "-h"] {
        let output = pane().arg(flag).output().unwrap();
        assert!(output.status.success(), "{flag}: {:?}", output.status);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"), "{flag}: {stdout}");
        assert!(stdout.contains("pane session --root <path>"));
        assert!(stdout.contains("pane ruler run"));
        assert!(stdout.contains("starts a session in the current project"));
        assert!(output.stderr.is_empty(), "{flag} wrote to stderr");
    }
}

#[test]
fn unknown_commands_and_options_fail_instead_of_echoing_stdin() {
    for argument in ["does-not-exist", "--does-not-exist"] {
        let output = pane().arg(argument).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{argument}");
        assert!(output.stdout.is_empty(), "{argument} wrote to stdout");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("unknown"), "{argument}: {stderr}");
        assert!(stderr.contains(argument), "{argument}: {stderr}");
        assert!(stderr.contains("pane --help"), "{argument}: {stderr}");
    }
}

#[test]
fn bare_pane_runs_the_ordinary_session_in_its_current_directory() {
    let root = Scratch::new("bare");
    let output = pane().current_dir(root.path()).output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        root.path().join(".pane/rollout.jsonl").is_file(),
        "bare pane did not use the session's default rollout path"
    );
}

#[test]
fn explicit_session_version_and_ruler_dispatch_remain_available() {
    let root = Scratch::new("session");
    let session = pane()
        .args(["session", "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    assert!(session.status.success());
    assert!(root.path().join(".pane/rollout.jsonl").is_file());

    let version = pane().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("pane {}\n", env!("CARGO_PKG_VERSION"))
    );

    let ruler = pane().arg("ruler").output().unwrap();
    assert!(!ruler.status.success());
    assert!(String::from_utf8_lossy(&ruler.stderr).contains("usage: pane ruler run [flags]"));
}
