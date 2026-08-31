//! Map lines 1796, 1516, 1531, 1401, 1402, 1403 and 1558 — **on the shipped
//! binary**.
//!
//! # Why every test here runs `glasshouse`, and none of them builds a
//! `Destination`
//!
//! `tests/session_router.rs` and `tests/routing_capability.rs` already prove
//! the tier gate, the tier-fit term and the capability term at the library
//! level, with destinations they construct themselves. Those tests pass
//! unchanged on the build this package started from — a build where
//! `Destination::tier_ceiling` was `None` on **every** destination the binary
//! made, because nothing in configuration, the provider registry or the
//! harness adapters stated a resource's tier. The gate was live and inert,
//! and the evidence ledger said so (`phase-34d.md`, *lines outside Phase
//! 34D*).
//!
//! So the only thing that can distinguish "the ceiling has a producer" from
//! "the ceiling type exists" is a test that writes a `config.toml`, runs the
//! binary, and reads what it printed. That is practice §35's rule applied to
//! a *value* rather than to a call: a producer no test reaches through the
//! production entry point is, to the test suite, not a producer. Delete the
//! `.with_tier_ceiling(ceiling)` line from `main.rs::routing_destinations`
//! and every test in this file that names a rejection or a `+0.400` fails,
//! while the whole library suite stays green.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The provider credential variable. A name only — nothing here resolves a
/// value.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TIER_CEILING_TEST_KEY";

/// A task text `classify_heuristically` reads as **standard**-tier work
/// needing repository access: `refactor` is a code-modification keyword and
/// `this project` is a repository reference, and neither a shell nor a
/// browser keyword appears. Written out rather than derived from the
/// classifier so that a mutation of the classifier fails these tests instead
/// of rescaling with them (practice §80 case 6).
const STANDARD_REPO_TASK: &str = "refactor the launch profile handling in this project";

/// A task text the same classifier reads as a pure question: **leaf** tier,
/// and no hard capability at all. The control condition for the two
/// capability tests — the same destinations, ranked with the requirement
/// removed.
const LEAF_QUESTION_TASK: &str = "what is the difference between a fresh and an existing session";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// A project whose user config is `version = 1`, one fake executable per
    /// harness in `harnesses`, and then `extra` verbatim.
    ///
    /// Every profile, provider and ceiling a test needs goes in `extra`, so
    /// each test states its own world in one readable block and no test can
    /// be broken by another one widening a shared fixture.
    fn new(harnesses: &[&str], extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");

        let mut config = String::from("version = 1\n\n");
        for harness in harnesses {
            let exe = install_fake_harness(&bin_dir, harness);
            let escaped = exe.display().to_string().replace('\\', "\\\\");
            config.push_str(&format!(
                "[integrations.{harness}]\nenabled = true\nexecutable = \"{escaped}\"\n\n"
            ));
        }
        config.push_str(extra);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(config_dir.join("config.toml"), config).expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-tier-ceiling-value")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn route(&self, args: &[&str]) -> String {
        let output = self.glasshouse(args);
        assert!(
            output.status.success(),
            "`glasshouse {}` failed:\n{}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, harness: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(format!("fake-{harness}"));
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, harness: &str) -> PathBuf {
    let path = bin_dir.join(format!("fake-{harness}.cmd"));
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// The block of a `glasshouse route` report that names one destination and
/// the terms it was scored on — from the line carrying `id` down to the
/// blank line that ends its explanation.
///
/// The report renders the winner, then `alternatives`, then `rejected`, and
/// every destination's terms are indented under its own heading. Slicing on
/// the id is what lets a test assert *this* destination's `workload tier
/// fit`, rather than asserting that the string appears somewhere in a report
/// that holds several.
fn block_for<'a>(report: &'a str, id: &str) -> &'a str {
    let start = report
        .find(id)
        .unwrap_or_else(|| panic!("`{id}` is not in the report:\n{report}"));
    let rest = &report[start..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    &rest[..end]
}

/// The signed magnitude the report printed for `term` inside `id`'s block.
///
/// The renderer writes `  {:+.3}  {name} — {evidence}`, so a line is split
/// into exactly those three parts and the **name is compared whole**.
/// Matching on a substring would be wrong here in a way that reads as a
/// passing test: `"capability fit"` is a substring of
/// `"harness capability fit"`, which is printed first and is almost always
/// `+0.000` — a test that took the first containing line would silently
/// assert about a different term than the one it names.
fn term_magnitude(report: &str, id: &str, term: &str) -> f64 {
    let block = block_for(report, id);
    let line = block
        .lines()
        .find(|line| named_term(line) == Some(term))
        .unwrap_or_else(|| panic!("`{id}` has no `{term}` term:\n{block}"));
    let number = line
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("`{term}` line has no magnitude: {line}"));
    number
        .parse()
        .unwrap_or_else(|_| panic!("`{number}` is not a magnitude: {line}"))
}

/// The contribution name a rendered explanation line carries, or `None` for
/// a line that is not one.
fn named_term(line: &str) -> Option<&str> {
    let (_, rest) = line.trim_start().split_once("  ")?;
    let (name, _) = rest.split_once(" — ")?;
    Some(name)
}

/// The destination the report recommends, read off its own `destination`
/// heading rather than off the first identifier that happens to appear.
fn chosen(report: &str) -> String {
    let line = report
        .lines()
        .find(|line| line.starts_with("destination "))
        .unwrap_or_else(|| panic!("the report names no destination:\n{report}"));
    line.split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("the destination heading carries no identifier: {line}"))
        .to_owned()
}

/// Two claude-code profiles on one provider, one of whose models the user
/// capped at `leaf` and one of whose models they said nothing about.
///
/// The two destinations are identical in harness, provider, protocol,
/// warmth, capacity and cost — the *only* difference between them is which
/// model they name, and therefore whether `providers.alpha.model_ceilings`
/// says anything about them.
fn capped_and_uncapped(ceilings: &str) -> Fixture {
    Fixture::new(
        &["claude-code"],
        &format!(
            "[providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\
             {ceilings}\n\
             [profiles.capped]\nharness = \"claude-code\"\nmodel = \"small\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.capped.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n\n\
             [profiles.uncapped]\nharness = \"claude-code\"\nmodel = \"big\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.uncapped.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        ),
    )
}

/// The `model_ceilings` table naming `small` a leaf-tier model, in the TOML
/// shape a user actually writes.
const SMALL_IS_LEAF: &str = "\n[providers.alpha.model_ceilings]\nsmall = \"leaf\"\n";

// --- 1796 + 1516: the producer, and the gate it makes live ----------------

/// **Lines 1796 and 1516.** A ceiling the user configured excludes a
/// destination the classifier's required tier is above, on the binary.
///
/// The whole chain is exercised: `providers.alpha.model_ceilings` parsed by
/// `ProviderConfig`, resolved by `EffectiveConfig::model_ceiling`, attached
/// by `main.rs::routing_destinations` through `Destination::with_tier_ceiling`,
/// read by `session::hard_constraint`, and rendered under `rejected` with
/// `HardConstraint::WorkloadTier`'s own reason.
///
/// The uncapped destination in the same run is the control: it differs only
/// in the model it names, and it is *not* rejected — so the rejection is the
/// ceiling's doing and not the task's.
#[test]
fn a_configured_ceiling_excludes_a_destination_below_the_required_tier_on_the_shipped_binary() {
    let fixture = capped_and_uncapped(SMALL_IS_LEAF);
    let report = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);

    let rejected = report
        .split_once("\nrejected\n")
        .unwrap_or_else(|| panic!("nothing was rejected at all:\n{report}"))
        .1;
    assert!(
        rejected.contains("fresh:claude-code:capped"),
        "the capped destination must be refused, not merely scored low:\n{report}"
    );
    assert!(
        rejected.contains("hard workload tier constraint"),
        "the refusal must name the workload-tier constraint:\n{report}"
    );
    assert!(
        rejected.contains(
            "the task needs at least the `standard` tier and this destination is established \
             to offer at most `leaf`"
        ),
        "a person reading this has to be told both tiers, or the refusal is unanswerable:\
         \n{report}"
    );
    assert!(
        !rejected.contains("fresh:claude-code:uncapped"),
        "the destination the user capped nothing on must still be eligible — the only \
         difference between the two is the model name the ceiling map keys on:\n{report}"
    );
}

/// **Line 1516's other half, and the rule the whole design rests on.** With
/// the identical project and no `model_ceilings` table, nothing is rejected
/// and the fit term reads *not established*.
///
/// "Nobody has said" is not "cannot": a build that rejected on an absent
/// ceiling would make every project that has never configured one lose
/// destinations, which is the opposite of what line 1516 asks for.
#[test]
fn without_a_ceiling_nothing_is_excluded_and_the_fit_term_says_not_established() {
    let fixture = capped_and_uncapped("");
    let report = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);

    assert!(
        !report.contains("hard workload tier constraint"),
        "no ceiling is configured, so no destination may be refused for its tier:\n{report}"
    );
    for id in ["fresh:claude-code:capped", "fresh:claude-code:uncapped"] {
        assert_eq!(
            term_magnitude(&report, id, "workload tier fit"),
            0.0,
            "an unstated ceiling must score zero, not a penalty:\n{report}"
        );
        assert!(
            block_for(&report, id).contains("nothing has established"),
            "and it must say so in words, so the reader can tell an unknown ceiling from a \
             low one:\n{report}"
        );
    }
}

/// **Line 1531.** Two destinations alike in every other term rank by their
/// tier fit: an exact ceiling (`+0.400`) above one with headroom
/// (`+0.200`).
///
/// The winner is deliberately the profile that sorts **second** —
/// `routing_destinations` offers fresh destinations in `profile_names`'
/// order and `SessionRouter::choose` uses caller order as its tiebreaker, so
/// a test whose expected winner came first could not tell the tier term from
/// the tiebreaker.
#[test]
fn tier_fit_orders_two_otherwise_equal_destinations() {
    let fixture = Fixture::new(
        &["claude-code"],
        &format!(
            "[providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
             [providers.alpha.model_ceilings]\nroomy = \"frontier\"\nfitted = \"standard\"\n\n\
             [profiles.a-roomy]\nharness = \"claude-code\"\nmodel = \"roomy\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.a-roomy.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n\n\
             [profiles.z-fitted]\nharness = \"claude-code\"\nmodel = \"fitted\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.z-fitted.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        ),
    );
    let report = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);

    assert_eq!(
        term_magnitude(&report, "fresh:claude-code:z-fitted", "workload tier fit"),
        0.4,
        "a ceiling equal to the required tier is the fit the router should prefer:\n{report}"
    );
    assert_eq!(
        term_magnitude(&report, "fresh:claude-code:a-roomy", "workload tier fit"),
        0.2,
        "a ceiling above the required tier can do the work, and spends headroom doing \
         it:\n{report}"
    );
    assert_eq!(
        chosen(&report),
        "fresh:claude-code:z-fitted",
        "the exactly-fitting destination must win, and it is offered second — so caller \
         order cannot be what chose it:\n{report}"
    );
}

/// **The other attachment site.** `routing_destinations` builds destinations
/// twice — once per recorded session and once per launch profile — and the
/// four tests above all reach the *fresh* one. A warm session takes its
/// ceiling from the model **it is actually running** (`SessionRecord.model`,
/// which `destination_backend` prefers over re-deriving one from the
/// profile), so it is a different read through a different value and needs
/// its own evidence: §35's rule is about a call site, and there are two.
///
/// The session is made by launching the fake harness under the capped
/// profile, which exits 0 and therefore leaves a resumable session behind.
#[test]
fn a_warm_sessions_ceiling_comes_from_the_model_it_is_running() {
    // Its own fixture: this profile has to be *launchable*, so it declares
    // the protocol Claude Code actually serves. The three tests above use
    // `openai-chat` deliberately — that is what makes their destinations
    // `ProtocolFit::Compatible` and keeps them scored rather than refused —
    // and `glasshouse launch` refuses it, which is `launch_can_resolve_protocol`
    // doing its job rather than a defect.
    let fixture = Fixture::new(
        &["claude-code"],
        &format!(
            "[providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\
             {SMALL_IS_LEAF}\n\
             [profiles.capped]\nharness = \"claude-code\"\nmodel = \"small\"\n\
             expected_protocol = \"anthropic-messages\"\n\
             [profiles.capped.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        ),
    );

    let launched =
        fixture.glasshouse(&["launch", "claude-code", "--headless", "--profile", "capped"]);
    assert!(
        launched.status.success(),
        "the launch that creates the warm session must succeed:\n{}{}",
        String::from_utf8_lossy(&launched.stdout),
        String::from_utf8_lossy(&launched.stderr),
    );

    let report = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);
    let rejected = report
        .split_once("\nrejected\n")
        .unwrap_or_else(|| panic!("nothing was rejected at all:\n{report}"))
        .1;
    assert!(
        rejected.contains("(existing)"),
        "the recorded session — not only the fresh profile — must be refused for its \
         model's ceiling, or the existing-session attachment site is untested:\n{report}"
    );
    assert!(
        rejected.contains(
            "the task needs at least the `standard` tier and this destination is established \
             to offer at most `leaf`"
        ),
        "and for the same stated reason:\n{report}"
    );
}

// --- 1401/1402/1403: tier and capability are two independent requirements --

/// The two-harness world lines 1402 and 1403 are about.
///
/// `a-cheap` is OpenCode, whose adapter declares `code_editing` as
/// `Unverified`, on a model the user marked **free** and capped at
/// `frontier` — the cheapest destination here and the one with the most raw
/// headroom. `z-capable` is Codex, whose adapter declares `code_editing`
/// verified present, on a metered model capped at `standard`.
///
/// Both are real production declarations (`harness::codex`,
/// `harness::opencode`), not fixture values, and `a-cheap` is offered first.
fn cheap_versus_capable() -> Fixture {
    Fixture::new(
        &["codex", "opencode"],
        &format!(
            "[providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\
             free_models = [\"cheap\"]\n\n\
             [providers.alpha.model_ceilings]\ncheap = \"frontier\"\ncapable = \"standard\"\n\n\
             [profiles.a-cheap]\nharness = \"opencode\"\nmodel = \"cheap\"\n\
             [profiles.a-cheap.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n\n\
             [profiles.z-capable]\nharness = \"codex\"\nmodel = \"capable\"\n\
             [profiles.z-capable.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        ),
    )
}

/// **Lines 1401, 1402 and 1403.** A task that requires a *lower* reasoning
/// tier but a *specific* capability goes to the capable destination, even
/// though the other one is free and has more raw headroom.
///
/// This is a two-condition experiment rather than a single ranking, which is
/// what makes it evidence rather than a coincidence. The destinations,
/// harnesses, providers, ceilings and prices are **identical** in both runs;
/// the only thing that changes is the task:
///
/// - with a leaf-tier question that names no capability, the free
///   frontier-ceiling destination wins;
/// - with a standard-tier task that requires repository access, the metered
///   standard-ceiling destination wins.
///
/// Whatever constant difference the two harnesses have contributes equally to
/// both runs and therefore cannot be what flipped the answer. What changed is
/// the two requirements, and they moved **independently**: the tier term went
/// from a tie to `+0.400` against `+0.200`, and the capability term from a
/// tie at zero to `+0.400` against zero. That independence is line 1401; the
/// lower tier with a capability still honoured is 1402; the capability
/// outranking the cheaper, higher-headroom model is 1403.
#[test]
fn a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier() {
    let fixture = cheap_versus_capable();

    let control = fixture.route(&["route", "--task", LEAF_QUESTION_TASK]);
    assert_eq!(
        chosen(&control),
        "fresh:opencode:a-cheap",
        "with nothing required beyond a leaf tier, the free destination is the one to \
         take:\n{control}"
    );
    for id in ["fresh:opencode:a-cheap", "fresh:codex:z-capable"] {
        assert_eq!(
            term_magnitude(&control, id, "capability fit"),
            0.0,
            "the control task must require no capability at all, or it is not a \
             control:\n{control}"
        );
    }

    let required = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);
    assert_eq!(
        term_magnitude(&required, "fresh:codex:z-capable", "capability fit"),
        0.4,
        "Codex declares code editing present, and the task needs repository access:\n{required}"
    );
    assert_eq!(
        term_magnitude(&required, "fresh:opencode:a-cheap", "capability fit"),
        0.0,
        "OpenCode declares nothing about code editing — not established, which is not a \
         `no` and must not be scored as one:\n{required}"
    );
    assert_eq!(
        term_magnitude(&required, "fresh:codex:z-capable", "workload tier fit"),
        0.4,
        "the capable destination is capped exactly at the tier the task needs:\n{required}"
    );
    assert_eq!(
        term_magnitude(&required, "fresh:opencode:a-cheap", "workload tier fit"),
        0.2,
        "the cheap destination has the *more* capable ceiling, and still loses:\n{required}"
    );
    assert_eq!(
        term_magnitude(&required, "fresh:opencode:a-cheap", "cost preference"),
        0.0,
        "and it is the free one, so price is pulling the other way:\n{required}"
    );
    assert_eq!(
        chosen(&required),
        "fresh:codex:z-capable",
        "a required capability must outrank a cheaper model with more raw headroom \
         (lines 1402, 1403):\n{required}"
    );
}

// --- 1558: cheapest among the candidates that satisfy the requirements -----

/// **Line 1558.** Between two candidates that satisfy the required tier and
/// the required capabilities equally, the one that costs nothing wins.
///
/// Same harness, same provider, same protocol, same ceiling, both fresh:
/// every other term in the explanation is equal by construction, and the
/// user's own `free_models` list is the only difference. The free profile is
/// offered **second**, so caller order is not what chose it.
#[test]
fn the_cheapest_healthy_adequate_candidate_wins() {
    let fixture = Fixture::new(
        &["claude-code"],
        &format!(
            "[providers.alpha]\ntemplate = \"openrouter\"\n\
             credential_env = [\"{CREDENTIAL_VAR}\"]\n\
             free_models = [\"thrifty\"]\n\n\
             [providers.alpha.model_ceilings]\npricey = \"standard\"\nthrifty = \"standard\"\n\n\
             [profiles.a-pricey]\nharness = \"claude-code\"\nmodel = \"pricey\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.a-pricey.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n\n\
             [profiles.z-thrifty]\nharness = \"claude-code\"\nmodel = \"thrifty\"\n\
             expected_protocol = \"openai-chat\"\n\
             [profiles.z-thrifty.backend]\nkind = \"direct-provider\"\nprovider = \"alpha\"\n"
        ),
    );
    let report = fixture.route(&["route", "--task", STANDARD_REPO_TASK]);

    // Both satisfy the tier — neither is refused, and both fit exactly.
    assert!(
        !report.contains("hard workload tier constraint"),
        "both candidates are capped at the tier the task needs:\n{report}"
    );
    for id in ["fresh:claude-code:a-pricey", "fresh:claude-code:z-thrifty"] {
        assert_eq!(
            term_magnitude(&report, id, "workload tier fit"),
            0.4,
            "the two are equally adequate for the tier, which is what makes price the \
             remaining question:\n{report}"
        );
    }
    assert_eq!(
        term_magnitude(&report, "fresh:claude-code:a-pricey", "cost preference"),
        -0.1,
        "a metered candidate is preferred only over ones nothing else separated it \
         from:\n{report}"
    );
    assert_eq!(
        term_magnitude(&report, "fresh:claude-code:z-thrifty", "cost preference"),
        0.0,
        "a zero-cost candidate is never penalised for its price:\n{report}"
    );
    assert_eq!(
        chosen(&report),
        "fresh:claude-code:z-thrifty",
        "among equally adequate healthy candidates, the cheapest wins — and it is offered \
         second, so caller order did not choose it:\n{report}"
    );
}

/// **The preservation clause, and the one this package could most easily
/// have broken.** A `route` that states no task must render exactly what it
/// rendered before any of this existed: no tier gate consulted, no tier fit
/// term, and no cost-preference term.
///
/// Both new terms are pushed under `score`'s `if let Some(required)`, which
/// is `None` for every caller that classified nothing — so this asserts the
/// absence of the two names, in a project that *does* configure ceilings and
/// would therefore show them the moment the condition slipped.
#[test]
fn a_task_less_route_reads_exactly_as_it_did_before_ceilings_existed() {
    let fixture = capped_and_uncapped(SMALL_IS_LEAF);
    let report = fixture.route(&["route"]);

    assert!(
        report.contains("fresh:claude-code:capped"),
        "with no task there is no required tier, so the capped destination is not capped \
         out of anything:\n{report}"
    );
    for absent in [
        "workload tier fit",
        "cost preference",
        "hard workload tier constraint",
    ] {
        assert!(
            !report.contains(absent),
            "`{absent}` must not appear in a report for work nobody classified:\n{report}"
        );
    }
    // And the report is still the one it always was.
    assert!(
        report.contains("the task named no hard capability requirement"),
        "`capability_fit`'s own no-task evidence string is the byte-level marker that this \
         path is unchanged:\n{report}"
    );
}
