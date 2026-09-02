//! Map line 2035: a seeded, offline proof fixture for the context firewall's
//! deterministic ladder (`firewall::reduce::reduce`) — reproducible without
//! any provider, and without the shipped binary: this file calls the ladder
//! directly, the same door `crates/glasshouse/src/firewall/reduce.rs`'s own
//! `#[cfg(test)]` module already uses.
//!
//! # What this proves, and what it does not
//!
//! [`ratio_table_recomputed_from_reduce_is_byte_identical`] recomputes
//! `ratios.txt` from the checked-in corpus under
//! `tests/fixtures/firewall/` and asserts it byte-identical — so a change to
//! any ladder rule (duplicate-line collapsing, blank-run collapsing, blob
//! elision) changes a number in that table and fails this test, on any
//! machine, with no provider involved. **The fixture measures the ladder;
//! it never tunes it** — nothing here feeds a ratio back into
//! `firewall::reduce`.
//!
//! [`regenerate_fixtures`] is `#[ignore]`d: it is the seeded generator that
//! produced the checked-in corpus in the first place, kept here (rather
//! than deleted after one use) so a future reader can reproduce or extend
//! the corpus from the same seed — `cargo test -p glasshouse --test
//! firewall_ladder_proof -- --ignored regenerate_fixtures`. It is not part
//! of the normal test run and never touches disk unless explicitly invoked.

use std::path::PathBuf;

use glasshouse::firewall::estimate::estimate_tokens;
use glasshouse::firewall::reduce::reduce;

/// The generator's one seed — every sample below is a pure function of this
/// constant and its own position in [`SAMPLE_NAMES`], so the whole corpus is
/// reproducible from one number.
const SEED: u64 = 0x5EED_1234_ABCD_EF01;

const SAMPLE_NAMES: [&str; 5] = [
    "duplicate_hits",
    "repeated_log_progress",
    "blank_line_runs",
    "generated_noise_blob",
    "all_uncertain",
];

/// A minimal xorshift64* PRNG — no external crate, deterministic across
/// platforms and Rust versions, and small enough to read in one screen.
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        // xorshift64* requires a non-zero state.
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn next_range(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// Family 1 (map lines 1982/1983): duplicate search hits. Fifteen distinct
/// grep-style lines, each repeated three times at pseudo-random positions —
/// every repeat after a line's first occurrence must collapse.
fn generate_duplicate_hits(rng: &mut Xorshift64) -> String {
    let bases: Vec<String> = (0..15)
        .map(|i| format!("src/module_{i}.rs:{i}: distinct hit number {i}"))
        .collect();
    let mut lines: Vec<&str> = Vec::new();
    for base in &bases {
        for _ in 0..3 {
            lines.push(base.as_str());
        }
    }
    // A pseudo-random (but deterministic) shuffle via repeated swaps, so
    // repeats are not simply adjacent.
    for i in (1..lines.len()).rev() {
        let j = rng.next_range(i as u64 + 1) as usize;
        lines.swap(i, j);
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Family 2: repeated log/stack/progress lines — the shape a real build or
/// test run produces. A fixed rotation of boilerplate lines repeated many
/// times, plus five distinct lines scattered through (mirroring the
/// flagship needle case elsewhere, without duplicating that test).
fn generate_repeated_log_progress(rng: &mut Xorshift64) -> String {
    let boilerplate = [
        "INFO: heartbeat ok",
        "  at frame::dispatch (frame.rs:42)",
        "test progress: 10/500",
    ];
    let mut lines: Vec<String> = Vec::new();
    for i in 0..200 {
        lines.push(boilerplate[i % boilerplate.len()].to_owned());
    }
    for i in 0..5 {
        let position = rng.next_range(lines.len() as u64) as usize;
        lines.insert(position, format!("WARN: distinct diagnostic {i}"));
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Family 3 (map line 1982's blank-run rule): several distinct content
/// lines separated by runs of one to five blank lines — every run beyond
/// its first blank line must collapse.
fn generate_blank_line_runs(rng: &mut Xorshift64) -> String {
    let mut out = String::new();
    for i in 0..20 {
        out.push_str(&format!("section {i}: distinct content line\n"));
        let run = 1 + rng.next_range(5) as usize;
        for _ in 0..run {
            out.push('\n');
        }
    }
    out
}

/// Family 4 (the blob-elision rule): one long, unbroken, whitespace-free
/// line — a deterministic hex dump long enough to cross the ladder's own
/// `BLOB_MIN_CHARS` threshold — bracketed by ordinary distinct lines so the
/// sample also exercises the line-granular rule beside the blob rule.
fn generate_generated_noise_blob(rng: &mut Xorshift64) -> String {
    let mut blob = String::new();
    for _ in 0..300 {
        blob.push_str(&format!("{:04x}", rng.next_u64() & 0xffff));
    }
    format!("preamble: distinct line one\n{blob}\npostamble: distinct line two\n")
}

/// Family 5: everything the ladder must leave alone — thirty distinct
/// lines, no blanks, no repeats, no line long enough to be a blob. Must
/// survive whole: `forwarded == original`.
fn generate_all_uncertain(_rng: &mut Xorshift64) -> String {
    let mut out = String::new();
    for i in 0..30 {
        out.push_str(&format!(
            "uncertain finding {i}: nothing here repeats or blanks\n"
        ));
    }
    out
}

/// The seeded generator itself: one [`Xorshift64`] stream, seeded once from
/// [`SEED`], threaded through every sample in [`SAMPLE_NAMES`] order — so
/// re-running this function reproduces the exact corpus on disk, byte for
/// byte, on any machine.
fn generate_corpus() -> Vec<(&'static str, String)> {
    let mut rng = Xorshift64::new(SEED);
    vec![
        ("duplicate_hits", generate_duplicate_hits(&mut rng)),
        (
            "repeated_log_progress",
            generate_repeated_log_progress(&mut rng),
        ),
        ("blank_line_runs", generate_blank_line_runs(&mut rng)),
        (
            "generated_noise_blob",
            generate_generated_noise_blob(&mut rng),
        ),
        ("all_uncertain", generate_all_uncertain(&mut rng)),
    ]
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/firewall")
}

fn ratio_table(corpus: &[(&str, String)]) -> String {
    let mut table = String::from("sample,original_estimate,forwarded_estimate,ratio\n");
    for (name, original) in corpus {
        let reduction = reduce(original);
        let original_estimate = estimate_tokens(original);
        let forwarded_estimate = estimate_tokens(&reduction.forwarded);
        let ratio = if original_estimate == 0 {
            0.0
        } else {
            forwarded_estimate as f64 / original_estimate as f64
        };
        table.push_str(&format!(
            "{name},{original_estimate},{forwarded_estimate},{ratio:.4}\n"
        ));
    }
    table
}

/// The seeded generator regenerates the checked-in corpus byte-identically
/// — this is what makes the corpus reproducible from [`SEED`] rather than a
/// one-off blob nobody can re-derive.
#[test]
fn checked_in_corpus_matches_the_seeded_generator() {
    let corpus = generate_corpus();
    for (name, generated) in &corpus {
        let on_disk = std::fs::read_to_string(fixtures_dir().join(format!("{name}.txt")))
            .unwrap_or_else(|err| panic!("read the checked-in {name}.txt: {err}"));
        assert_eq!(
            generated, &on_disk,
            "the checked-in {name}.txt no longer matches what SEED regenerates; \
             re-run `cargo test -p glasshouse --test firewall_ladder_proof -- \
             --ignored regenerate_fixtures` and commit the result"
        );
    }
}

/// Map line 2035's own proof: `ratios.txt`, recomputed from the checked-in
/// corpus through the real `firewall::reduce::reduce`, is byte-identical to
/// what is on disk. This is the test the `ladder-drift` mutation must kill:
/// any change to the ladder's rules changes a forwarded-estimate figure and
/// this assertion fails.
#[test]
fn ratio_table_recomputed_from_reduce_is_byte_identical() {
    let mut corpus = Vec::new();
    for name in SAMPLE_NAMES {
        let text = std::fs::read_to_string(fixtures_dir().join(format!("{name}.txt")))
            .unwrap_or_else(|err| panic!("read the checked-in {name}.txt: {err}"));
        corpus.push((name, text));
    }
    let table = ratio_table(&corpus);
    let expected = std::fs::read_to_string(fixtures_dir().join("ratios.txt"))
        .expect("read the checked-in ratios.txt");
    assert_eq!(table, expected);
}

/// Map line 1983's own guarantee, pinned for the `all_uncertain` sample by
/// name: a corpus with nothing the ladder can positively prove is a repeat,
/// a blank run, or a blob must survive whole.
#[test]
fn the_all_uncertain_sample_survives_whole() {
    let original = std::fs::read_to_string(fixtures_dir().join("all_uncertain.txt"))
        .expect("read the checked-in all_uncertain.txt");
    let reduction = reduce(&original);
    assert_eq!(reduction.forwarded, original);
    assert_eq!(reduction.retained_candidates, reduction.total_candidates);
}

/// The seeded generator, checked in as a reusable test helper rather than a
/// one-off script — `#[ignore]`d because writing fixture files is not part
/// of a normal test run. Run explicitly to regenerate or extend the corpus
/// from [`SEED`]:
/// `cargo test -p glasshouse --test firewall_ladder_proof -- --ignored regenerate_fixtures`.
#[test]
#[ignore = "writes the checked-in fixture corpus; run explicitly to regenerate it"]
fn regenerate_fixtures() {
    let corpus = generate_corpus();
    std::fs::create_dir_all(fixtures_dir()).expect("create the fixtures directory");
    for (name, text) in &corpus {
        std::fs::write(fixtures_dir().join(format!("{name}.txt")), text)
            .unwrap_or_else(|err| panic!("write {name}.txt: {err}"));
    }
    let table = ratio_table(&corpus);
    std::fs::write(fixtures_dir().join("ratios.txt"), table).expect("write ratios.txt");
}
