//! Acceptance for map line 2455's grant derivation: `.claude/settings.json`
//! compiles to one immutable profile, and every pre-call path question is
//! answered from it. Each test names the invariant of
//! `docs/product/pane/sandbox-grants.md` it holds.
//!
//! Nothing here executes anything — map line 2457. The tests construct
//! settings documents and paths; no tool is run, no process is spawned.

use pane::project;
use pane::sandbox::profile::{Access, PermissionDenied, Profile};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The production source, for the two invariants that are properties of the
/// code's shape rather than of any one call: §1.1 (nothing can widen a built
/// profile) and §1.4 (a refusal is a value, never a question).
const SOURCE: &str = include_str!("../src/sandbox/profile.rs");

/// This repository's own `.claude/settings.json`, which `sandbox-grants.md`
/// §2 names as the fixture. Pinned at compile time so the test decides on
/// the bytes that are checked in.
const REPOSITORY_SETTINGS: &str = include_str!("../../../.claude/settings.json");

/// A throwaway project directory, removed when the test finishes.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "pane-sandbox-test-{}-{label}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    /// The root as a pattern writes it: forward slashes, so a settings
    /// document embedding it needs no JSON escaping.
    fn pattern_root(&self) -> String {
        self.root.to_string_lossy().replace('\\', "/")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn home() -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            return PathBuf::from(value);
        }
    }
    panic!("neither HOME nor USERPROFILE is set; the never-grantable set is defined against it");
}

fn refusal(result: Result<(), PermissionDenied>) -> PermissionDenied {
    result.expect_err("expected a refusal, got a grant")
}

/// §1.2. A `deny` refuses a path even when a longer, more exact `allow` names
/// it: there is no most-specific-wins rule to reason about.
#[test]
fn a_deny_beats_a_more_specific_allow() {
    let fixture = Fixture::new("deny-beats-allow");
    let root = fixture.pattern_root();
    let settings = format!(
        r#"{{"permissions":{{
            "allow":["Read({root}/secrets/token.txt)","Edit({root}/secrets/token.txt)"],
            "deny":["Read({root}/secrets/**)"]
        }}}}"#
    );
    let profile = Profile::compile(&fixture.root, Some(&settings));
    let exact = fixture.root.join("secrets/token.txt");

    for access in [Access::Read, Access::Write] {
        let denied = refusal(profile.check("Read", access, &exact));
        assert!(
            denied.rule.contains("permissions.deny"),
            "the deciding rule must be the deny entry, got {:?}",
            denied.rule
        );
        assert!(
            denied.rule.contains("secrets/**"),
            "the refusal must quote the deny pattern, got {:?}",
            denied.rule
        );
    }

    // The allow is real: a sibling the deny does not name is still granted.
    profile
        .check("Read", Access::Read, &fixture.root.join("notes.md"))
        .expect("a path no deny names is still readable");
}

/// §1.3. The project root is the only writable root by default -- not the
/// home directory, not a temp directory, not the project's parent.
#[test]
fn nothing_outside_the_project_root_is_writable_by_default() {
    let fixture = Fixture::new("writable-root");
    let profile = Profile::compile(&fixture.root, Some(r#"{"permissions":{}}"#));

    profile
        .check("Write", Access::Write, &fixture.root.join("build/out.txt"))
        .expect("the project root is writable");

    let outside = [
        fixture.root.parent().unwrap().join("escaped.txt"),
        std::env::temp_dir().join("pane-sandbox-elsewhere.txt"),
        home().join("scratch-that-is-not-the-project.txt"),
        PathBuf::from("/etc/hosts"),
    ];
    for path in outside {
        let denied = refusal(profile.check("Write", Access::Write, &path));
        assert!(
            denied
                .rule
                .contains("the project root is the only writable root"),
            "{path:?} refused with the wrong rule: {:?}",
            denied.rule
        );
    }
}

/// §4. Every entry is refusable by no pattern at all -- not merely absent
/// from the defaults. Each case below writes a settings document that tries
/// to grant the entry and asserts the profile is unmoved.
#[test]
fn a_settings_document_cannot_grant_the_never_grantable_set() {
    let fixture = Fixture::new("never-grantable");
    let home = home();
    let home_pattern = home.to_string_lossy().replace('\\', "/");

    // §4.1 network. A pattern that names a domain registers nothing at all.
    let network = Profile::compile(
        &fixture.root,
        Some(r#"{"permissions":{"allow":["WebFetch(domain:example.com)","WebFetch"]}}"#),
    );
    assert!(!network.grants_network(), "network is never granted");
    assert!(!network.admits_mcp_tool("WebFetch"));
    assert_eq!(
        network.rule_count(),
        0,
        "a network pattern is not a file rule"
    );

    // §4.2 the keyring, §4.3 the named $HOME paths, §4.4 Glasshouse's own
    // state. Each is allowed as broadly as a document can spell it.
    let settings = format!(
        r#"{{"permissions":{{"allow":[
            "Read({home_pattern}/Library/Keychains/**)",
            "Edit({home_pattern}/Library/Keychains/**)",
            "Read({home_pattern}/.gnupg/**)",
            "Read({home_pattern}/.local/share/keyrings/**)",
            "Read({home_pattern}/.claude/**)","Edit({home_pattern}/.claude/**)",
            "Read({home_pattern}/.codex/**)",
            "Read({home_pattern}/.ssh/id_ed25519)",
            "Read({home_pattern}/.aws/credentials)",
            "Read({home_pattern}/.config/**)",
            "Read({home_pattern}/.local/share/glasshouse/**)",
            "Edit({home_pattern}/.local/share/glasshouse/memory.db)",
            "Read({home_pattern}/Library/Application Support/glasshouse/**)",
            "Read({home_pattern}/.glasshouse/**)"
        ]}}}}"#
    );
    let profile = Profile::compile(&fixture.root, Some(&settings));

    let never: [(PathBuf, &str); 11] = [
        (home.join("Library/Keychains/login.keychain-db"), "§4.2"),
        (home.join(".gnupg/secring.gpg"), "§4.2"),
        (home.join(".local/share/keyrings/login.keyring"), "§4.2"),
        (home.join(".claude/settings.json"), "§4.3"),
        (home.join(".codex/auth.json"), "§4.3"),
        (home.join(".ssh/id_ed25519"), "§4.3"),
        (home.join(".aws/credentials"), "§4.3"),
        (home.join(".config/gh/hosts.yml"), "§4.3"),
        (home.join(".local/share/glasshouse/memory.db"), "§4.4"),
        (
            home.join("Library/Application Support/glasshouse/state.db"),
            "§4.4",
        ),
        (home.join(".glasshouse/routing.db"), "§4.4"),
    ];
    for (path, section) in never {
        for access in [Access::Read, Access::Write] {
            let denied = refusal(profile.check("Read", access, &path));
            assert!(
                denied.rule.contains("never grantable by any pattern"),
                "{path:?} was refused for the wrong reason: {:?}",
                denied.rule
            );
            assert!(
                denied.rule.contains(section),
                "{path:?} should cite {section}, got {:?}",
                denied.rule
            );
        }
    }

    // And the same set against the broadest pattern a document can spell.
    // A `deny` is not what refuses these -- there is none here -- so this is
    // the case that separates "never grantable" from "not granted by default".
    let maximal = Profile::compile(
        &fixture.root,
        Some(r#"{"permissions":{"allow":["Read(/**)","Write(/**)","Edit(/**)","Read(**)"]}}"#),
    );
    assert!(maximal.rule_count() >= 4, "the maximal allow did compile");
    maximal
        .check("Read", Access::Read, Path::new("/etc/hosts"))
        .expect("the maximal allow is real; it grants an ordinary path");
    for path in [
        home.join(".ssh/id_ed25519"),
        home.join(".aws/credentials"),
        home.join("Library/Keychains/login.keychain-db"),
        home.join(".local/share/glasshouse/memory.db"),
        fixture.root.join(".claude/settings.json"),
    ] {
        let denied = refusal(maximal.check("Read", Access::Write, &path));
        assert!(
            denied.rule.contains("never")
                || denied.rule.contains("never writable")
                || denied.rule.contains("§1.5"),
            "{path:?} was widened by a maximal allow: {:?}",
            denied.rule
        );
    }
    assert!(!maximal.grants_network());

    // §4.6 process-level escapes, refused however broadly `Bash` is granted.
    let escapes = Profile::compile(
        &fixture.root,
        Some(
            r#"{"permissions":{"allow":["Bash","Bash(sandbox-exec*)","Bash(bwrap*)","Bash(lldb*)"]}}"#,
        ),
    );
    for command in [
        "sandbox-exec -p '(version 1)(allow default)' /bin/sh",
        "bwrap --dev-bind / / /bin/sh",
        "/usr/bin/lldb -p 4242",
        "sudo gdb --pid 4242",
        "sh -c \"sandbox-exec -p x /bin/sh\"",
    ] {
        let denied = refusal(escapes.admits_command(command));
        assert!(
            denied.rule.contains("§4.6"),
            "{command:?} was refused for the wrong reason: {:?}",
            denied.rule
        );
    }
    escapes
        .admits_command("cargo test -p pane")
        .expect("a bare Bash still admits an ordinary command line");
}

/// §1.1 and §1.5. The profile is built once and there is no expression that
/// widens it afterwards.
///
/// The type makes the direct thing uncompilable -- there is no public field,
/// no setter and no method taking a mutable receiver to call -- so a test
/// cannot write the widening it is asserting against. The assertion is
/// therefore twofold: the source is scanned for every shape that would make
/// one possible, and the nearest observable thing is checked -- a profile
/// keeps its answers when the document it came from is rewritten underneath
/// it, which is exactly the widening §1.5 exists to prevent.
#[test]
fn the_profile_cannot_be_widened_after_it_is_built() {
    let header = "pub struct Profile {";
    let fields_start = SOURCE.find(header).expect("the Profile struct") + header.len();
    let fields_end = fields_start + SOURCE[fields_start..].find("\n}").expect("its end");
    let declaration = &SOURCE[fields_start..fields_end];
    assert!(
        !declaration.contains("pub "),
        "Profile has a public field, which is a way to widen it: {declaration}"
    );

    for shape in [
        "&mut self",
        "pub fn set_",
        "RefCell",
        "Cell<",
        "Mutex",
        "RwLock",
        "static mut",
        "unsafe",
    ] {
        assert!(
            !SOURCE.contains(shape),
            "`{shape}` appears in the profile's source; a built profile must have no way to widen"
        );
    }

    let fixture = Fixture::new("no-widening");
    let elsewhere = std::env::temp_dir().join(format!(
        "pane-sandbox-elsewhere-{}/secret.txt",
        std::process::id()
    ));
    std::fs::create_dir_all(fixture.root.join(".claude")).unwrap();
    std::fs::write(
        fixture.root.join(".claude/settings.json"),
        r#"{"permissions":{"allow":[]}}"#,
    )
    .unwrap();

    let profile = Profile::from_project(&project::load(&fixture.root));
    refusal(profile.check("Read", Access::Read, &elsewhere));

    // The document is rewritten to grant it -- the one thing a program with
    // write access to the project root could do.
    let widened = format!(
        r#"{{"permissions":{{"allow":["Read({}/**)"]}}}}"#,
        elsewhere
            .parent()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    );
    std::fs::write(fixture.root.join(".claude/settings.json"), &widened).unwrap();

    refusal(profile.check("Read", Access::Read, &elsewhere));

    // And the rewrite was real: a profile compiled afresh does grant it, so
    // the assertion above is about the profile's immutability rather than
    // about a document that never took effect.
    Profile::compile(&fixture.root, Some(&widened))
        .check("Read", Access::Read, &elsewhere)
        .expect("a freshly compiled profile honours the rewritten document");
}

/// §1.5. `.claude/` lives inside the writable project root, so a program that
/// could write it could widen the profile it was derived from.
#[test]
fn dot_claude_is_not_writable_even_though_it_is_inside_the_project() {
    let fixture = Fixture::new("dot-claude");
    let root = fixture.pattern_root();
    let settings = format!(
        r#"{{"permissions":{{"allow":["Write({root}/.claude/**)","Edit({root}/.claude/settings.json)"]}}}}"#
    );
    let profile = Profile::compile(&fixture.root, Some(&settings));
    let settings_path = fixture.root.join(".claude/settings.json");

    let denied = refusal(profile.check("Write", Access::Write, &settings_path));
    assert!(
        denied.rule.contains("§1.5"),
        "the refusal must cite the invariant, got {:?}",
        denied.rule
    );

    // It stays readable: settings.json is read before the sandbox is entered.
    profile
        .check("Read", Access::Read, &settings_path)
        .expect(".claude is readable; only writing it is refused");
}

/// §2. `Bash(cargo test*)` grants `cargo test` no file access whatsoever. It
/// admits a command line; the process it spawns gets exactly the grants the
/// `Read`/`Write`/`Edit` patterns produced.
#[test]
fn a_bash_pattern_grants_no_file_access() {
    let fixture = Fixture::new("bash-argv");
    let profile = Profile::compile(
        &fixture.root,
        Some(r#"{"permissions":{"allow":["Bash(cargo test*)"]}}"#),
    );

    assert_eq!(
        profile.rule_count(),
        0,
        "a Bash pattern contributes nothing to the filesystem profile"
    );
    assert_eq!(profile.command_pattern_count(), 1);

    profile
        .admits_command("cargo test -p pane")
        .expect("the command line is admitted");
    refusal(profile.admits_command("cargo build"));

    // Admitting the command line grants no path outside the project root.
    for path in [
        std::env::temp_dir().join("pane-bash-elsewhere.txt"),
        PathBuf::from("/etc/hosts"),
    ] {
        let denied = refusal(profile.check("Read", Access::Read, &path));
        assert!(denied.rule.contains("no grant covers this path"));
    }
}

/// §4.1. A network-needing tool is absent, not present and failing: the
/// pattern registers nothing and is not carried as a disabled rule.
#[test]
fn a_webfetch_pattern_registers_nothing() {
    let fixture = Fixture::new("webfetch");
    let without = Profile::compile(&fixture.root, Some(r#"{"permissions":{"allow":[]}}"#));
    let with = Profile::compile(
        &fixture.root,
        Some(
            r#"{"permissions":{"allow":["WebFetch(domain:example.com)"],"deny":["WebFetch(domain:evil.example)"]}}"#,
        ),
    );

    assert_eq!(with.rule_count(), without.rule_count());
    assert_eq!(
        with.command_pattern_count(),
        without.command_pattern_count()
    );
    assert_eq!(with.mcp_tool_count(), 0);
    assert!(
        with.diagnostics().is_empty(),
        "a WebFetch pattern leaves no disabled rule behind: {:?}",
        with.diagnostics()
    );
    assert!(!with.grants_network());
    assert!(!with.admits_mcp_tool("WebFetch"));
}

/// §1.4 and §5. A refusal is a value naming the deciding rule, and no code
/// path in the module can ask anybody anything.
#[test]
fn a_refusal_names_the_deciding_rule_and_never_prompts() {
    for shape in [
        "stdin",
        "read_line",
        "prompt",
        "escalat",
        "println!",
        "eprintln!",
    ] {
        assert!(
            !SOURCE.contains(shape),
            "`{shape}` appears in the profile's source; a refusal is a value, not a question"
        );
    }

    let fixture = Fixture::new("refusal");
    let root = fixture.pattern_root();
    let settings =
        format!(r#"{{"permissions":{{"deny":["Read({root}/vault/provider-keys.env)"]}}}}"#);
    let profile = Profile::compile(&fixture.root, Some(&settings));

    let denied = refusal(profile.check(
        "read",
        Access::Read,
        &fixture.root.join("vault/provider-keys.env"),
    ));
    assert_eq!(denied.tool, "read");
    assert!(denied.path.ends_with("vault/provider-keys.env"));
    assert!(
        denied.rule.contains("provider-keys.env") && denied.rule.contains("permissions.deny"),
        "the deny entry must be quoted back: {:?}",
        denied.rule
    );

    let absent = refusal(profile.check("read", Access::Read, Path::new("/etc/shadow")));
    assert_eq!(
        absent.rule,
        "no grant covers this path; the project root is the only readable root"
    );

    // The path is reported as it was resolved, which on a host where `/etc`
    // is a symlink is `/private/etc/shadow` -- the spelling the decision was
    // actually made on, so a person can reproduce it.
    assert!(absent.path.ends_with("etc/shadow"), "{:?}", absent.path);
    let rendered = absent.to_string();
    assert_eq!(
        rendered,
        format!(
            "PermissionDenied: read(\"{}\")\n  rule: {}\n  tool: read",
            absent.path, absent.rule
        )
    );
    assert!(rendered.contains("\n  rule: no grant covers this path"));
    assert!(rendered.ends_with("\n  tool: read"));
}

/// §2. Two spellings of one path are how a containment check comes to
/// disagree with itself, so every candidate is compared after `~` expansion,
/// relative resolution and symlink resolution.
///
/// The symlink half is exercised through a symlink the host already has
/// rather than one this test creates: `std::os::unix::fs::symlink` is the
/// only portable-enough way to make one and it needs a `#[cfg]`, which this
/// package does not use. On macOS `std::env::temp_dir()` is `/var/folders/…`
/// and `/var` is a symlink to `/private/var`, so the unresolved and resolved
/// spellings below are genuinely different paths there; elsewhere the case
/// degenerates to an equality and the `..` and `~` halves carry the test.
#[test]
fn two_spellings_of_one_path_decide_the_same_way() {
    let fixture = Fixture::new("spellings");
    std::fs::create_dir_all(fixture.root.join("secrets")).unwrap();
    std::fs::write(fixture.root.join("secrets/token.txt"), "t").unwrap();
    let root = fixture.pattern_root();
    let settings = format!(r#"{{"permissions":{{"deny":["Read({root}/secrets/**)"]}}}}"#);
    let profile = Profile::compile(&fixture.root, Some(&settings));

    let direct = fixture.root.join("secrets/token.txt");
    let traversed = fixture.root.join("build/../secrets/./token.txt");
    let canonical = std::fs::canonicalize(&direct).unwrap();

    let deciding = refusal(profile.check("Read", Access::Read, &direct)).rule;
    for spelling in [&traversed, &canonical] {
        let denied = refusal(profile.check("Read", Access::Read, spelling));
        assert_eq!(
            denied.rule, deciding,
            "{spelling:?} decided differently from {direct:?}"
        );
        assert_eq!(
            denied.path,
            refusal(profile.check("Read", Access::Read, &direct)).path
        );
    }

    // The same, the other way round: a profile built from the unresolved
    // spelling of the root agrees with one built from the resolved spelling.
    let resolved_root = std::fs::canonicalize(&fixture.root).unwrap();
    let from_resolved = Profile::compile(&resolved_root, Some(&settings));
    assert_eq!(
        refusal(from_resolved.check("Read", Access::Read, &direct)).rule,
        deciding
    );
    from_resolved
        .check("Write", Access::Write, &fixture.root.join("build/out.txt"))
        .expect("the project root is the same root under either spelling");

    // `~` expands before matching, so a home path is never mistaken for a
    // project-relative one.
    let tilde = refusal(profile.check("Read", Access::Read, Path::new("~/.ssh/id_ed25519")));
    assert!(
        tilde.rule.contains("§4.3"),
        "`~` must expand before matching, got {:?}",
        tilde.rule
    );
}

/// A settings document is untrusted input from a directory the profile makes
/// writable. One that cannot be parsed grants nothing rather than everything.
#[test]
fn a_malformed_settings_document_grants_nothing() {
    let fixture = Fixture::new("malformed");
    let outside = std::env::temp_dir().join("pane-sandbox-malformed-target.txt");

    for document in [
        "{ this is not json",
        "",
        "[]",
        r#"{"permissions":"everything"}"#,
        r#"{"permissions":{"allow":"Read(/)"}}"#,
        r#"{"permissions":{"allow":[42,null]}}"#,
    ] {
        let profile = Profile::compile(&fixture.root, Some(document));
        assert_eq!(
            profile.rule_count(),
            0,
            "{document:?} produced a filesystem rule"
        );
        assert_eq!(profile.command_pattern_count(), 0);
        assert_eq!(profile.mcp_tool_count(), 0);
        assert!(!profile.grants_network());
        refusal(profile.check("Read", Access::Read, &outside));

        // The defaults are not the document's to remove either: the project
        // root stays readable and writable.
        profile
            .check("Write", Access::Write, &fixture.root.join("out.txt"))
            .expect("the project root is writable whatever the document said");
    }

    // A document that is merely unparseable says so, so a person can see why
    // their grants vanished.
    let broken = Profile::compile(&fixture.root, Some("{ this is not json"));
    assert!(
        broken
            .diagnostics()
            .iter()
            .any(|line| line.contains("not valid JSON")),
        "{:?}",
        broken.diagnostics()
    );
}

/// A pattern kind the profile does not understand grants nothing -- never
/// everything -- and says so.
#[test]
fn an_unknown_pattern_kind_grants_nothing() {
    let fixture = Fixture::new("unknown-kind");
    let profile = Profile::compile(
        &fixture.root,
        Some(
            r#"{"permissions":{
                "allow":["Sudo(*)","Read","Edit","NotebookEdit(**)","Read()","Grant(/)","mcp__ledger__query"],
                "ask":["Read(/etc/**)"],
                "additionalDirectories":["/"],
                "defaultMode":"acceptEdits"
            }}"#,
        ),
    );

    assert_eq!(
        profile.rule_count(),
        0,
        "no unknown pattern produced a file rule"
    );
    assert_eq!(profile.command_pattern_count(), 0);
    assert_eq!(
        profile.mcp_tool_count(),
        1,
        "the one known kind still lands"
    );
    assert!(profile.admits_mcp_tool("mcp__ledger__query"));

    let diagnostics = profile.diagnostics().join("\n");
    for expected in [
        "Sudo",
        "NotebookEdit",
        "Grant",
        "a bare `Read`",
        "permissions.ask",
        "permissions.additionalDirectories",
        "permissions.defaultMode",
    ] {
        assert!(
            diagnostics.contains(expected),
            "no diagnostic mentions {expected}: {diagnostics}"
        );
    }

    refusal(profile.check(
        "Read",
        Access::Read,
        &std::env::temp_dir().join("pane-sandbox-unknown-target.txt"),
    ));
}

/// The fixture `sandbox-grants.md` §2 names: this repository's own settings
/// document, compiled as written.
#[test]
fn the_repositorys_own_settings_document_compiles_to_its_written_grants() {
    let fixture = Fixture::new("repository-fixture");
    let profile = Profile::compile(&fixture.root, Some(REPOSITORY_SETTINGS));

    assert_eq!(
        profile.rule_count(),
        9,
        "the fixture carries seven allow and two deny entries"
    );
    assert!(
        profile.diagnostics().is_empty(),
        "every entry mapped to a rule: {:?}",
        profile.diagnostics()
    );
    assert_eq!(profile.command_pattern_count(), 0, "`hooks` is not a grant");

    let runtime = Path::new("/Users/eneas/projects/glasshouse/.agent-runtime");
    profile
        .check("Read", Access::Read, &runtime.join("report-x.md"))
        .expect("Read(report-*.md) is granted");
    profile
        .check("Write", Access::Write, &runtime.join("report-x.md"))
        .expect("Write(report-*.md) is granted");

    let read_only = refusal(profile.check("Write", Access::Write, &runtime.join("packet-x.md")));
    assert!(read_only.rule.contains("only writable root"));

    for secret in ["provider-keys.env", "anything.env"] {
        let denied = refusal(profile.check("Read", Access::Read, &runtime.join(secret)));
        assert!(
            denied.rule.contains("permissions.deny"),
            "{secret} must be refused by the deny entry: {:?}",
            denied.rule
        );
    }

    // The case-sensitivity decision, stated once in the source and asserted
    // here: a `deny` matches case-insensitively on every platform, so a
    // case-insensitive filesystem cannot walk a secret past it.
    let denied = refusal(profile.check("Read", Access::Read, &runtime.join("SECRET.ENV")));
    assert!(
        denied.rule.contains("permissions.deny"),
        "{:?}",
        denied.rule
    );

    // An `allow` matches case-sensitively on every platform, so the same
    // trick cannot reach a path its author never spelled.
    refusal(profile.check("Read", Access::Read, &runtime.join("REPORT-X.MD")));
}

/// The escape direction, which `GH-PANE-61D-PROFILE` named as its own
/// thinnest claim and could not test: a symlink **inside** the project root
/// whose target is **outside** it.
///
/// Its `two_spellings_of_one_path_decide_the_same_way` only exercises
/// symlinks the host already happens to have (`/var` → `/private/var` on
/// macOS), which degenerates to an equality on Linux and proves nothing
/// there. The worker took the weaker test rather than reach for the `#[cfg]`
/// its packet forbade, and said so — correctly, because that prohibition
/// existed to keep it out of 61D's platform appliers, not out of its own
/// tests. The lead's review is the right place to spend it, and a test may
/// name a platform where production code here may not.
///
/// This is also `sandbox-grants.md` §6's
/// `symlink_targets_outside_the_project_root_are_rejected_by_project_config_io`
/// in miniature — the Phase 46 boundary, asked of pane's own profile.
#[cfg(unix)]
#[test]
fn a_symlink_inside_the_root_pointing_out_of_it_is_refused_for_write() {
    let fixture = Fixture::new("symlink-escape");

    // **Canonical spellings throughout, deliberately.** On macOS `temp_dir()`
    // is under `/var`, which is itself a symlink to `/private/var`, so a
    // fixture that used the raw path would be refused for a spelling mismatch
    // whether or not the profile resolved anything -- and the test would pass
    // against a build that had stopped resolving. Measured: with the raw
    // spelling, replacing `resolve(...)` with `path.to_path_buf()` left this
    // test green while breaking eight others.
    let canonical_root = std::fs::canonicalize(&fixture.root).unwrap();
    let root = canonical_root.to_string_lossy().replace('\\', "/");

    // Somewhere genuinely outside the project, with a real file in it.
    let outside = Fixture::new("symlink-escape-target");
    std::fs::write(outside.root.join("stolen.txt"), b"outside the project").unwrap();

    // The project grants itself everything it possibly can.
    let settings = format!(
        r#"{{"permissions":{{"allow":["Read({root}/**)","Write({root}/**)","Edit({root}/**)"]}}}}"#
    );
    let profile = Profile::compile(&canonical_root, Some(&settings));

    // A path inside the root, spelled canonically, that resolves outside it.
    let link = canonical_root.join("escape");
    std::os::unix::fs::symlink(&outside.root, &link).unwrap();
    let through_link = link.join("stolen.txt");

    // Sanity: the link really does leave the project.
    let canonical = std::fs::canonicalize(&through_link).unwrap();
    assert!(
        through_link.starts_with(&canonical_root),
        "fixture is wrong: the link's own spelling must be inside the root"
    );
    assert!(
        !canonical.starts_with(&canonical_root),
        "fixture is wrong: {canonical:?} did not escape the root"
    );

    for access in [Access::Read, Access::Write] {
        let denied = refusal(profile.check("Write", access, &through_link));
        assert!(
            !denied.rule.is_empty(),
            "a refusal must name the deciding rule"
        );
    }

    // And the same path spelled canonically decides the same way, so the
    // grant cannot be recovered by choosing a spelling.
    refusal(profile.check("Write", Access::Write, &canonical));
}
