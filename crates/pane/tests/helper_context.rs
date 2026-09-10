use pane::helper_context::{EvidenceKind, HelperRole, PreparedContext, prepare};
use pane::sandbox::profile::Profile;
use pane::tools::invoke::CancellationToken;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pane-helper-context-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn profile(&self) -> Profile {
        Profile::compile(&self.root, Some(r#"{"permissions":{}}"#))
    }

    fn prepare(&self, role: HelperRole, input: &str) -> PreparedContext {
        prepare(role, input, &self.profile(), &CancellationToken::new())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn tree(packet: &PreparedContext) -> &str {
    packet
        .evidence
        .iter()
        .find(|item| item.kind == EvidenceKind::Tree)
        .map(|item| item.text.as_str())
        .unwrap_or("")
}

fn pattern_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn helper_names_map_only_to_the_three_preparation_roles() {
    assert_eq!(
        HelperRole::from_helper_name("find"),
        Some(HelperRole::Scout)
    );
    assert_eq!(
        HelperRole::from_helper_name("check"),
        Some(HelperRole::Checker)
    );
    assert_eq!(
        HelperRole::from_helper_name("reduce"),
        Some(HelperRole::Reducer)
    );
    assert_eq!(HelperRole::from_helper_name("other"), None);
}

#[test]
fn scout_is_sorted_role_specific_and_prunes_generated_secret_and_huge_content() {
    let fixture = Fixture::new("scout");
    std::fs::write(
        fixture.root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n[dev-dependencies]\n",
    )
    .unwrap();
    std::fs::write(fixture.root.join("z.rs"), "fn unrelated() {}\n").unwrap();
    std::fs::write(fixture.root.join("a.rs"), "fn needle_handler() {}\n").unwrap();
    std::fs::write(fixture.root.join(".env"), "TOKEN=do-not-render\n").unwrap();
    std::fs::write(fixture.root.join("large.rs"), vec![b'x'; 300 * 1024]).unwrap();
    for generated in ["node_modules", "build", "vendor"] {
        let directory = fixture.root.join(generated);
        std::fs::create_dir(&directory).unwrap();
        for index in 0..300 {
            std::fs::write(directory.join(format!("noise-{index:03}.rs")), "needle\n").unwrap();
        }
    }

    let packet = fixture.prepare(HelperRole::Scout, "find the needle handler");
    let listed = tree(&packet);
    assert!(listed.find("a.rs") < listed.find("z.rs"), "{listed}");
    assert!(listed.contains("Cargo.toml"), "{packet:#?}");
    assert!(!listed.contains("node_modules"), "{listed}");
    assert!(!listed.contains("build/"), "{listed}");
    assert!(!listed.contains("vendor/"), "{listed}");
    assert!(
        !packet.rendered.contains("do-not-render"),
        "{}",
        packet.rendered
    );
    assert!(
        !packet.rendered.contains("large.rs\n"),
        "{}",
        packet.rendered
    );
    assert!(
        packet
            .omissions
            .iter()
            .any(|item| { item.subject == "large.rs" && item.reason == "file size limit reached" })
    );
    assert!(
        packet
            .evidence
            .iter()
            .any(|item| { item.kind == EvidenceKind::Match && item.subject == "a.rs:1" })
    );
    assert!(
        packet
            .rendered
            .starts_with("Deterministic starting evidence.")
    );
    assert!(packet.rendered.contains("untrusted data"));
    assert_eq!(
        packet,
        fixture.prepare(HelperRole::Scout, "find the needle handler"),
        "unchanged inputs must produce identical prepared evidence"
    );
}

#[test]
fn scout_applies_supported_gitignore_and_reports_unsupported_scope() {
    let fixture = Fixture::new("ignore");
    std::fs::write(fixture.root.join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir(fixture.root.join("ignored")).unwrap();
    std::fs::write(fixture.root.join("ignored/hidden.rs"), "needle\n").unwrap();
    std::fs::create_dir(fixture.root.join("kept")).unwrap();
    std::fs::write(fixture.root.join("kept/visible.rs"), "needle\n").unwrap();
    std::fs::write(fixture.root.join("kept/.gitignore"), "!visible.rs\n").unwrap();

    let packet = fixture.prepare(HelperRole::Scout, "find needle");
    assert!(!tree(&packet).contains("ignored/hidden.rs"), "{packet:#?}");
    assert!(!tree(&packet).contains("kept/visible.rs"), "{packet:#?}");
    assert!(
        packet.omissions.iter().any(|item| {
            item.subject == "kept" && item.reason.contains("unsupported gitignore")
        }),
        "{packet:#?}"
    );
}

#[test]
fn scout_stops_at_wide_and_deep_directories_with_explicit_omissions() {
    let fixture = Fixture::new("bounds");
    let wide = fixture.root.join("wide");
    std::fs::create_dir(&wide).unwrap();
    for index in 0..257 {
        std::fs::write(wide.join(format!("entry-{index:03}.rs")), "needle\n").unwrap();
    }
    let mut deep = fixture.root.join("deep");
    for _ in 0..9 {
        std::fs::create_dir_all(&deep).unwrap();
        deep = deep.join("next");
    }

    let packet = fixture.prepare(HelperRole::Scout, "find needle");
    assert!(
        packet.omissions.iter().any(|item| {
            item.subject == "wide" && item.reason == "directory width limit reached"
        }),
        "{packet:#?}"
    );
    assert!(
        packet
            .omissions
            .iter()
            .any(|item| item.reason == "depth limit reached"),
        "{packet:#?}"
    );
    assert!(!tree(&packet).contains("entry-"), "{packet:#?}");
}

#[test]
fn unreadable_ignore_semantics_never_turn_into_unfiltered_discovery() {
    let fixture = Fixture::new("oversized-ignore");
    std::fs::write(fixture.root.join(".gitignore"), "# padding\n".repeat(1000)).unwrap();
    std::fs::write(fixture.root.join("hidden.rs"), "needle-private\n").unwrap();
    let packet = fixture.prepare(HelperRole::Scout, "find needle");
    assert!(!packet.rendered.contains("needle-private"));
    assert!(!tree(&packet).contains("hidden.rs"));
    assert!(
        packet
            .omissions
            .iter()
            .any(|o| o.reason == "gitignore size or type unsupported")
    );
}

#[test]
fn discovery_does_not_derive_search_terms_beyond_the_input_byte_bound() {
    let fixture = Fixture::new("bounded-input");
    std::fs::write(
        fixture.root.join("match.rs"),
        "fn unique_tail_needle() {}\n",
    )
    .unwrap();
    let input = format!("{} unique_tail_needle", "x".repeat(1024 * 1024));
    let packet = fixture.prepare(HelperRole::Scout, &input);
    assert!(
        !packet
            .evidence
            .iter()
            .any(|item| item.kind == EvidenceKind::Match)
    );
    assert!(
        packet
            .omissions
            .iter()
            .any(|item| item.subject == "scout input" && item.reason == "input byte limit reached")
    );
}

#[test]
fn scout_obeys_parent_denial_and_never_follows_symlinks() {
    let fixture = Fixture::new("access");
    std::fs::create_dir(fixture.root.join("denied")).unwrap();
    std::fs::write(fixture.root.join("denied/secret.rs"), "needle-secret\n").unwrap();
    std::fs::write(fixture.root.join("visible.rs"), "needle-visible\n").unwrap();
    let denied = pattern_path(&fixture.root.join("denied"));
    let settings =
        format!(r#"{{"permissions":{{"deny":["Read({denied})","Read({denied}/**)"]}}}}"#);
    let profile = Profile::compile(&fixture.root, Some(&settings));

    let packet = prepare(
        HelperRole::Scout,
        "find needle",
        &profile,
        &CancellationToken::new(),
    );
    assert!(tree(&packet).contains("visible.rs"), "{packet:#?}");
    assert!(
        !packet.rendered.contains("needle-secret"),
        "{}",
        packet.rendered
    );
    assert!(
        packet.omissions.iter().any(|item| {
            item.subject == "denied" && item.reason == "parent profile denied read"
        }),
        "{packet:#?}"
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            fixture.root.join("visible.rs"),
            fixture.root.join("link.rs"),
        )
        .unwrap();
        let packet = fixture.prepare(HelperRole::Scout, "find needle");
        assert!(!tree(&packet).contains("link.rs"), "{packet:#?}");
        assert!(
            packet.omissions.iter().any(|item| {
                item.subject == "link.rs" && item.reason == "symbolic link not followed"
            }),
            "{packet:#?}"
        );
    }
}

#[test]
fn cancelled_preparation_reads_nothing_and_reports_cancellation() {
    let fixture = Fixture::new("cancelled");
    std::fs::write(fixture.root.join("visible.rs"), "needle\n").unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let packet = prepare(HelperRole::Scout, "find needle", &fixture.profile(), &token);
    assert!(packet.cancelled);
    assert!(packet.evidence.is_empty(), "{packet:#?}");
    assert!(packet.operations.is_empty(), "{packet:#?}");
    assert!(
        packet
            .omissions
            .iter()
            .any(|item| item.reason == "cancelled")
    );
}

#[test]
fn checker_uses_supplied_change_and_original_contract_without_running_tests() {
    let fixture = Fixture::new("checker");
    std::fs::write(
        fixture.root.join("README.md"),
        "The parser must preserve caller input exactly.\n",
    )
    .unwrap();
    let supplied = "diff --git a/src/parser.rs b/src/parser.rs\n--- a/src/parser.rs\n+++ b/src/parser.rs\n@@\n+TOKEN=hidden\n+preserve caller input\n";
    let packet = fixture.prepare(HelperRole::Checker, supplied);
    assert!(
        packet.evidence.iter().any(|item| {
            item.kind == EvidenceKind::ChangedFile && item.subject == "src/parser.rs"
        }),
        "{packet:#?}"
    );
    assert!(
        packet
            .evidence
            .iter()
            .any(|item| { item.kind == EvidenceKind::Contract && item.subject == "README.md" }),
        "{packet:#?}"
    );
    assert!(
        !packet.rendered.contains("TOKEN=hidden"),
        "{}",
        packet.rendered
    );
    assert!(
        !packet
            .operations
            .iter()
            .any(|item| item.action.contains("run"))
    );
}

#[test]
fn reducer_extracts_real_failure_windows_and_clean_logs_are_negative() {
    let fixture = Fixture::new("reducer");
    std::fs::write(
        fixture.root.join("must-not-read.rs"),
        "error: filesystem bait\n",
    )
    .unwrap();
    let failed = fixture.prepare(
        HelperRole::Reducer,
        "$ cargo test\ncompiling demo\nerror[E0308]: wrong type\n  detail\nexit code: 1\n",
    );
    let window = failed
        .evidence
        .iter()
        .find(|item| item.kind == EvidenceKind::Failure)
        .expect("failure window");
    assert!(window.text.contains("compiling demo"), "{window:#?}");
    assert!(window.text.contains("error[E0308]"), "{window:#?}");
    assert!(!failed.rendered.contains("filesystem bait"));
    assert!(
        !failed
            .operations
            .iter()
            .any(|item| item.action == "enumerate")
    );

    let clean = fixture.prepare(
        HelperRole::Reducer,
        "$ cargo test\ntest result: ok. 12 passed; 0 failed; 0 ignored\nexit code: 0\n",
    );
    assert!(
        !clean
            .evidence
            .iter()
            .any(|item| item.kind == EvidenceKind::Failure),
        "{clean:#?}"
    );
    assert!(
        clean.evidence.iter().any(|item| {
            item.kind == EvidenceKind::Status && item.text == "no failure marker found"
        }),
        "{clean:#?}"
    );
}
