use std::path::PathBuf;

use pane::sandbox::profile::Profile;
use pane::tools::exact_edit;
use sha2::{Digest, Sha256};

fn fixture(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("pane-exact-edit-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[test]
fn exact_versioned_edit_is_atomic_and_reports_hashes_and_lines() {
    let root = fixture("success");
    let path = root.join("file.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let profile = Profile::compile(&root, None);
    let result =
        exact_edit::apply(&profile, &path, &hash("one\ntwo\nthree\n"), "two", "2\nII").unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "one\n2\nII\nthree\n"
    );
    assert_eq!(result.before_sha256, hash("one\ntwo\nthree\n"));
    assert_eq!(result.after_sha256, hash("one\n2\nII\nthree\n"));
    assert_eq!(
        result.changed_lines,
        exact_edit::ChangedLines {
            start: 2,
            before: 1,
            after: 2
        }
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stale_missing_ambiguous_and_noop_edits_do_not_write() {
    let root = fixture("refusals");
    let path = root.join("file.txt");
    std::fs::write(&path, "same same\n").unwrap();
    let profile = Profile::compile(&root, None);
    // The stale case matches the whole file exactly once, so the version check
    // is the only thing that can refuse it: without it the edit would land.
    let stale = exact_edit::apply(&profile, &path, &hash("old"), "same same\n", "new").unwrap_err();
    assert_eq!(stale.kind, "stale_hash");
    assert_eq!(
        stale.message,
        "The source version changed; refresh context before editing."
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "same same\n");
    for result in [
        exact_edit::apply(&profile, &path, &hash("same same\n"), "missing", "new"),
        exact_edit::apply(&profile, &path, &hash("same same\n"), "same", "new"),
        exact_edit::apply(&profile, &path, &hash("same same\n"), "same", "same"),
    ] {
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "same same\n");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn denied_outside_and_invalid_utf8_targets_are_refused() {
    let root = fixture("confined");
    let outside = fixture("outside");
    let denied = root.join("denied.txt");
    let external = outside.join("external.txt");
    let binary = root.join("binary.txt");
    std::fs::write(&denied, "secret").unwrap();
    std::fs::write(&external, "outside").unwrap();
    std::fs::write(&binary, [0xff, 0xfe]).unwrap();
    let profile = Profile::compile(
        &root,
        Some(r#"{"permissions":{"deny":["Write(denied.txt)"]}}"#),
    );
    assert!(exact_edit::apply(&profile, &denied, &hash("secret"), "secret", "x").is_err());
    assert!(exact_edit::apply(&profile, &external, &hash("outside"), "outside", "x").is_err());
    assert!(exact_edit::apply(&profile, &binary, &hash_bytes(&[0xff, 0xfe]), "x", "y").is_err());
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
