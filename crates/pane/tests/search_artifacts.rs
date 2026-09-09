use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::sandbox::profile::Profile;
use pane::tools::invoke::{self, Args, ToolContext};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_prefix("pane-search-artifacts")
    }

    fn with_prefix(prefix: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for (relative, contents) in [
            ("src/lib.rs", "needle source\n"),
            (".pane/rollout.jsonl", "needle model feedback\n"),
            (".pane/config.toml", "needle pane config\n"),
            (".env", "needle environment\n"),
            (".settings/local.txt", "needle hidden config\n"),
            (".git/config", "needle git internals\n"),
            ("literal[1].txt", "-rf literal pattern\n"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        Self {
            root: std::fs::canonicalize(root).unwrap(),
        }
    }

    fn call(&self, tool: &str, args: Args) -> String {
        let profile = Profile::compile(&self.root, None);
        let glasshouse = Glasshouse::None;
        let session = SessionId::new("search-artifact-test");
        invoke::run(
            &ToolContext {
                profile: &profile,
                glasshouse: &glasshouse,
                session: &session,
            },
            tool,
            &args,
        )
        .unwrap()
        .stdout
    }

    fn glob(&self, pattern: &str, path: Option<&Path>) -> String {
        let mut args = Args::new().with("pattern", pattern);
        if let Some(path) = path {
            args = args.with("path", path.to_string_lossy());
        }
        self.call("glob", args)
    }

    fn grep(&self, pattern: &str, path: Option<&Path>) -> String {
        let mut args = Args::new().with("pattern", pattern);
        if let Some(path) = path {
            args = args.with("path", path.to_string_lossy());
        }
        self.call("grep", args)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn broad_search_omits_generated_feedback_but_keeps_normal_hidden_files() {
    let fixture = Fixture::new();
    let glob = fixture.glob("**/*", None);
    for retained in [
        "src/lib.rs",
        ".pane/config.toml",
        ".env",
        ".settings/local.txt",
    ] {
        assert!(
            glob.contains(&fixture.root.join(retained).display().to_string()),
            "{retained}: {glob}"
        );
    }
    assert!(!glob.contains("rollout.jsonl"), "{glob}");
    assert!(!glob.contains(".git"), "{glob}");

    let grep = fixture.grep("needle", None);
    for retained in [
        "src/lib.rs",
        ".pane/config.toml",
        ".env",
        ".settings/local.txt",
    ] {
        assert!(
            grep.contains(&fixture.root.join(retained).display().to_string()),
            "{retained}: {grep}"
        );
    }
    assert!(!grep.contains("model feedback"), "{grep}");
    assert!(!grep.contains("git internals"), "{grep}");
}

#[test]
fn a_large_self_matching_rollout_cannot_starve_the_real_source_match() {
    let fixture = Fixture::new();
    let rollout = fixture.root.join(".pane/rollout.jsonl");
    std::fs::write(&rollout, "needle repeated model feedback\n".repeat(4_000)).unwrap();

    let grep = fixture.grep("needle", None);
    assert!(
        grep.contains(&fixture.root.join("src/lib.rs").display().to_string()),
        "real source match was lost: {grep}"
    );
    assert!(grep.contains("needle source"), "{grep}");
    assert!(!grep.contains("rollout.jsonl"), "{grep}");
    assert!(!grep.contains("model feedback"), "{grep}");
}

#[test]
fn a_colon_in_the_project_path_cannot_hide_the_rollout_prefix() {
    // A colon inside a path *component* is a unix-only spelling: Windows
    // reserves `:` for the drive separator and refuses the name outright
    // (`ERROR_INVALID_NAME`). The colon this test needs is already in
    // every absolute Windows path, so the default fixture supplies it.
    #[cfg(unix)]
    let fixture = Fixture::with_prefix("pane:colon-search-artifacts");
    #[cfg(not(unix))]
    let fixture = Fixture::new();
    let grep = fixture.grep("needle", None);
    assert!(
        grep.contains(&fixture.root.join("src/lib.rs").display().to_string()),
        "{grep}"
    );
    assert!(!grep.contains("rollout.jsonl"), "{grep}");
    assert!(!grep.contains("model feedback"), "{grep}");
}

#[test]
fn explicit_hidden_targets_opt_back_in_without_changing_read_authority() {
    let fixture = Fixture::new();
    let pane = fixture.glob(".pane/*", None);
    assert!(pane.contains("rollout.jsonl"), "{pane}");
    assert!(pane.contains("config.toml"), "{pane}");
    assert!(
        !pane.contains(".git"),
        "a .pane opt-in also exposed Git: {pane}"
    );
    let git = fixture.glob(".git/*", None);
    assert!(
        git.contains(&fixture.root.join(".git/config").display().to_string()),
        "{git}"
    );

    let rollout = fixture.root.join(".pane/rollout.jsonl");
    let explicit = fixture.grep("model feedback", Some(&rollout));
    assert!(explicit.contains("model feedback"), "{explicit}");

    let read = fixture.call("read", Args::new().with("path", rollout.to_string_lossy()));
    assert_eq!(read, "needle model feedback\n");
}

#[test]
fn literal_glob_and_option_shaped_grep_patterns_remain_data() {
    let fixture = Fixture::new();
    let literal = fixture.glob("literal[1].txt", None);
    assert!(literal.contains("literal[1].txt"), "{literal}");
    let option = fixture.grep("-rf", None);
    assert!(option.contains("literal[1].txt"), "{option}");
}
