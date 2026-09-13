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

/// Several hunks are one mutation: located against the original text,
/// installed once, and reported per hunk in the caller's order.
#[test]
fn multi_hunk_edit_applies_every_hunk_against_the_original_text() {
    let root = fixture("hunks");
    let path = root.join("file.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
    let profile = Profile::compile(&root, None);
    // Given out of file order, so the result proves hunks keep the caller's
    // order while the text is rebuilt in offset order.
    let result = exact_edit::apply_hunks(
        &profile,
        &path,
        &hash("one\ntwo\nthree\nfour\n"),
        &["four".to_string(), "one".to_string()],
        &["4\nIV".to_string(), "1".to_string()],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "1\ntwo\nthree\n4\nIV\n"
    );
    assert_eq!(result.hunks.len(), 2);
    assert_eq!(
        result.hunks[0],
        exact_edit::ChangedLines {
            start: 4,
            before: 1,
            after: 2
        }
    );
    assert_eq!(
        result.hunks[1],
        exact_edit::ChangedLines {
            start: 1,
            before: 1,
            after: 1
        }
    );
    assert_eq!(result.changed_lines, result.hunks[0]);
    assert_eq!(result.after_sha256, hash("1\ntwo\nthree\n4\nIV\n"));
    let _ = std::fs::remove_dir_all(root);
}

/// Every refusal names the hunk and the reason, and writes nothing.
#[test]
fn multi_hunk_refusals_name_the_hunk_and_leave_the_file_untouched() {
    let root = fixture("hunk-refusals");
    let path = root.join("file.txt");
    let text = "one\ntwo\ntwo\nthree\n";
    std::fs::write(&path, text).unwrap();
    let profile = Profile::compile(&root, None);
    let s = |items: &[&str]| {
        items
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
    };
    let cases: Vec<(Vec<String>, Vec<String>, &str, &str)> = vec![
        (
            s(&["one", "three"]),
            s(&["1"]),
            "hunk_count_mismatch",
            "2 hunk(s)",
        ),
        (s(&[]), s(&[]), "hunk_count_mismatch", "at least one"),
        (
            s(&["one", "absent"]),
            s(&["1", "?"]),
            "missing_match",
            "hunk 1",
        ),
        (
            s(&["one", "two"]),
            s(&["1", "2"]),
            "ambiguous_match",
            "hunk 1",
        ),
        (
            s(&["one\ntwo", "two\ntwo"]),
            s(&["a", "b"]),
            "overlapping_hunks",
            "hunks 0 and 1",
        ),
        (
            s(&["one", "three"]),
            s(&["1", "three"]),
            "edit_refused",
            "hunk 1",
        ),
        (s(&["one", ""]), s(&["1", "x"]), "edit_refused", "hunk 1"),
    ];
    for (olds, replacements, kind, names) in cases {
        let error = exact_edit::apply_hunks(&profile, &path, &hash(text), &olds, &replacements)
            .unwrap_err();
        assert_eq!(error.kind, kind, "{olds:?}: {error:?}");
        assert!(error.message.contains(names), "{olds:?}: {error:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
    // The version check is the same one the single form makes.
    let stale = exact_edit::apply_hunks(&profile, &path, &hash("old"), &s(&["one"]), &s(&["1"]))
        .unwrap_err();
    assert_eq!(stale.kind, "stale_hash");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let _ = std::fs::remove_dir_all(root);
}

/// The single form reports its one hunk under `hunks` too, so a reader of
/// the result has one field to look at.
#[test]
fn the_single_form_reports_its_hunk_in_both_fields() {
    let root = fixture("single-hunks");
    let path = root.join("file.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let profile = Profile::compile(&root, None);
    let result = exact_edit::apply(&profile, &path, &hash("one\ntwo\n"), "two", "2").unwrap();
    assert_eq!(result.hunks, vec![result.changed_lines.clone()]);
    let _ = std::fs::remove_dir_all(root);
}
