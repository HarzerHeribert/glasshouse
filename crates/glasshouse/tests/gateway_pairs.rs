//! `glasshouse gateway pairs`, through the shipped binary: `field_rows()`'s
//! first production caller, giving the cache and effort dispositions
//! (capability map line 2014's reason and the 2039 prerequisite) a place
//! they are actually rendered.

use std::path::PathBuf;
use std::process::Command;

use glasshouse::gateway::translate;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    /// Run `glasshouse gateway pairs` against an isolated data and config
    /// directory that carries no configuration file and no keychain entry —
    /// no configuration directory or secret store this command could read.
    fn pairs_output(&self) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("gateway")
            .arg("pairs")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable");
        assert!(
            output.status.success(),
            "glasshouse gateway pairs must succeed with no configuration and no keychain \
             entry: stderr {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

#[test]
fn every_pair_and_its_refusal_reason_is_named() {
    let fixture = Fixture::new();
    let stdout = fixture.pairs_output();
    for pair in translate::pairs() {
        match pair.refusal() {
            None => {
                let line = format!("{} -> {}: supported", pair.from, pair.to);
                assert!(
                    stdout.contains(&line),
                    "missing line: {line}\n---\n{stdout}"
                );
            }
            Some(reason) => {
                let line = format!("{} -> {}: refused ({reason})", pair.from, pair.to);
                assert!(
                    stdout.contains(&line),
                    "missing line: {line}\n---\n{stdout}"
                );
            }
        }
    }
}

#[test]
fn every_codec_has_cache_and_effort_lines_and_a_named_refused_field() {
    let fixture = Fixture::new();
    let stdout = fixture.pairs_output();
    let mut saw_a_codec = false;
    for protocol in translate::PROTOCOLS {
        let Some(rows) = translate::field_rows(protocol) else {
            continue;
        };
        saw_a_codec = true;
        // Anchored on the two-space indent and the trailing colon so a
        // field name that merely ends in "effort" (`reasoning_effort:`)
        // cannot satisfy this the way a bare `contains("effort:")` would.
        assert!(
            stdout.contains("\n  cache:"),
            "no `  cache:` line anywhere for a protocol with a codec ({protocol})\n---\n{stdout}"
        );
        assert!(
            stdout.contains("\n  effort:"),
            "no `  effort:` line anywhere for a protocol with a codec ({protocol})\n---\n{stdout}"
        );
        if let Some((field, reason)) = rows.refused.first() {
            let line = format!("refuses {field}: {reason}");
            assert!(
                stdout.contains(&line),
                "missing refused-field line for {protocol}: {line}\n---\n{stdout}"
            );
        }
    }
    assert!(saw_a_codec, "expected at least one protocol with a codec");
}

#[test]
fn the_command_succeeds_with_no_configuration_and_no_keychain() {
    let fixture = Fixture::new();
    let stdout = fixture.pairs_output();
    assert!(stdout.contains("PAIRS"));
    assert!(stdout.contains("FIELDS"));
}
