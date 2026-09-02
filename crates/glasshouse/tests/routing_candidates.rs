//! `GH-CANDIDATE-PROOFS` — proof-only regression tests for map lines
//! 1513 (protocol/tool half), 1520 (entitlement half) and 1521 (end to
//! end), reached through the library's public API rather than
//! `main.rs`'s private generators. See `.agent-runtime/report-recon-35a.md`
//! Cause 3 for the production caller chain each test pins.

use std::collections::BTreeMap;
use std::time::Instant;

use glasshouse::config::{EffectiveConfig, ProfileConfig, UserConfig};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::free::FreePool;
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{
    AssignedModel, Backend, Cost, CredentialId, Entitlement, EntitlementRules, HardConstraint,
    ToolSemantics,
};
use glasshouse::secret::SecretRef;

fn no_overrides() -> PairingOverrides {
    PairingOverrides::from_parts("no configuration", BTreeMap::new(), BTreeMap::new())
}

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_KEY", provider.to_uppercase()),
        },
    )
}

fn backend(provider: &str, protocol: &str, tools: ToolSemantics) -> Backend {
    Backend::new(
        provider,
        protocol,
        AssignedModel::named("some-model"),
        credential(provider),
        Cost::Metered,
        tools,
    )
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: no_overrides(),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    fn inputs(&self, needs_tool_calls: bool) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements {
                needs_tool_calls,
                ..TaskRequirements::default()
            },
        }
    }
}

// ---------------------------------------------------------------------
// 1513 — the protocol/tool half of "fresh gateway-backed candidates only
// as installed-harness profiles whose protocol and tool semantics match".
//
// The gate (`session::hard_constraint`) is not gateway-specific code: it
// applies uniformly to every `Destination`, regardless of which backend
// produced it. These tests exercise that uniform gate directly through
// `SessionRouter`, which is exactly how a gateway-backed destination
// built by `main.rs::destination_backend` would be checked.
// ---------------------------------------------------------------------

/// `IntegrationId::OpenCode` declares only `WireProtocol::OpenAiChat`
/// (`harness/opencode.rs`), and the gateway translation table has no
/// `openai-chat -> anthropic-messages` pair (`gateway/translate/mod.rs`,
/// `PairStatus::Refused(NOT_YET_REVERSE)`), so a destination whose backend
/// serves `anthropic-messages` is protocol-incompatible for it. The
/// census's mutation (drop the protocol check for a gateway-backed
/// destination) would let this candidate reach scoring instead.
#[test]
fn a_protocol_incompatible_destination_is_excluded_before_scoring_1513() {
    let fixture = Fixture::new();
    let incompatible = Destination::fresh(
        "incompatible",
        IntegrationId::OpenCode,
        "gateway",
        backend(
            "some-gateway",
            "anthropic-messages",
            ToolSemantics::Verified,
        ),
        None,
    );

    let inputs = fixture.inputs(false);
    let rejected = SessionRouter::new().refused(std::slice::from_ref(&incompatible), &inputs);

    assert_eq!(
        rejected.len(),
        1,
        "the protocol-incompatible destination must be hard-refused, not scored"
    );
    assert_eq!(rejected[0].0.id(), "incompatible");
    assert_eq!(
        rejected[0].1,
        HardConstraint::Protocol,
        "the refusal must name the protocol constraint specifically"
    );

    // The same gate lets a compatible destination through, proving the
    // exclusion above is about the protocol and not about something else
    // this test accidentally also changed.
    let compatible = Destination::fresh(
        "compatible",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::Verified),
        None,
    );
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&compatible),
            &inputs,
        )
        .expect("a protocol-compatible destination must be chosen when it is the only one");
    assert_eq!(routed.chosen().id(), "compatible");
}

/// The tool-semantics half of the same gate
/// (`session.rs:4838-4842`): a task that needs tool calls cannot be sent to
/// a destination established **not** to carry them, uniformly across
/// backend types. The census's mutation drops this check for a
/// gateway-backed destination specifically.
#[test]
fn a_tool_incompatible_destination_is_excluded_when_the_task_needs_tool_calls_1513() {
    let fixture = Fixture::new();
    let no_tools = Destination::fresh(
        "no-tools",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::KnownAbsent),
        None,
    );

    let inputs = fixture.inputs(true);
    let rejected = SessionRouter::new().refused(std::slice::from_ref(&no_tools), &inputs);

    assert_eq!(rejected.len(), 1);
    assert_eq!(
        rejected[0].1,
        HardConstraint::ToolSemantics,
        "a task needing tool calls must refuse a destination established not to carry them"
    );
}

// ---------------------------------------------------------------------
// 1520 — the entitlement half of "exclude candidates explicitly disabled
// or forbidden by user policy". The generation-time half (a disabled
// profile never reaches the offered set) is pinned in `main.rs`'s own
// test module; this is the post-generation hard exclusion
// (`Entitlement::constraint`, `session.rs:4818`, called from
// `hard_constraint`).
// ---------------------------------------------------------------------

/// A destination backed by an entitlement whose rules deny this harness is
/// excluded outright — refused, never merely scored lower. The census's
/// mutation (bypass `Entitlement::constraint`'s deny check) would let a
/// denied harness reach scoring instead of being refused.
#[test]
fn a_destination_backed_by_a_harness_denying_entitlement_is_excluded_not_scored_1520() {
    let fixture = Fixture::new();
    let denied_rules = EntitlementRules::UNRESTRICTED.deny_harnesses([IntegrationId::OpenCode]);
    let denied = Destination::fresh(
        "denied",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::Verified),
        None,
    )
    .with_entitlement(Some(Entitlement::new("policy-test", denied_rules)));

    let inputs = fixture.inputs(false);
    let rejected = SessionRouter::new().refused(std::slice::from_ref(&denied), &inputs);

    assert_eq!(
        rejected.len(),
        1,
        "a candidate whose entitlement forbids this harness must be excluded, not merely \
         disfavoured by scoring"
    );
    assert!(
        matches!(rejected[0].1, HardConstraint::Entitlement { .. }),
        "the exclusion must be attributed to the entitlement rule: {:?}",
        rejected[0].1
    );

    // The same rules admit a harness they do not deny, proving the
    // exclusion above is about the policy and not about entitlements
    // refusing everything unconditionally.
    let admitted_rules = EntitlementRules::UNRESTRICTED.deny_harnesses([IntegrationId::Codex]);
    let admitted = Destination::fresh(
        "admitted",
        IntegrationId::OpenCode,
        "gateway",
        backend("some-gateway", "openai-chat", ToolSemantics::Verified),
        None,
    )
    .with_entitlement(Some(Entitlement::new("policy-test", admitted_rules)));
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&admitted),
            &inputs,
        )
        .expect("a candidate whose entitlement does not deny this harness must be chosen");
    assert_eq!(routed.chosen().id(), "admitted");
}

// ---------------------------------------------------------------------
// 1521 — "keep at least one deterministic fallback candidate when a
// usable native session exists". The guarantee lives in
// `EffectiveConfig::profile_enabled`'s unconditional short-circuit for
// the implied Native profile (`config/mod.rs:5045-5048`), confirmed here
// end to end: even a user who writes a `[profiles.native]` entry with
// `enabled = false` — which does reach the profile table, since nothing
// stops that key from being configured — must still see it enabled, and
// a destination built from it must still survive `SessionRouter::choose`
// against a task with no tier requirement (per the packet: "usable" is
// doing real work in the line's own wording, so the proof stays inside
// what the line actually promises).
// ---------------------------------------------------------------------
#[test]
fn the_native_profile_survives_as_a_deterministic_fallback_end_to_end_1521() {
    let harness = IntegrationId::ClaudeCode;

    let mut user = UserConfig::default();
    let mut attempted_disable = ProfileConfig::new(harness);
    attempted_disable.set_enabled(false);
    user.profiles_mut()
        .set(glasshouse::profile::NATIVE_PROFILE_NAME, attempted_disable);

    let effective = EffectiveConfig::new(&user, None);
    assert!(
        effective
            .profile_enabled(glasshouse::profile::NATIVE_PROFILE_NAME)
            .value,
        "the implied Native profile must stay enabled even against a user config entry that \
         tries to disable it"
    );

    // A destination built the way `main.rs::destination_backend`'s
    // `BackendResource::Native` arm builds one: the harness's own
    // sign-in, always protocol/tool-compatible with its own harness.
    let native = Destination::fresh(
        "native",
        harness,
        glasshouse::profile::NATIVE_PROFILE_NAME,
        backend(
            harness.slug(),
            "anthropic-messages",
            ToolSemantics::Verified,
        ),
        None,
    );

    let fixture = Fixture::new();
    // No tier requirement — the packet's own instruction: the line does
    // not claim a profile whose ceiling is below a classified minimum
    // survives, so the proof stays inside what it actually promises.
    let inputs = fixture.inputs(false);
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            std::slice::from_ref(&native),
            &inputs,
        )
        .expect("the Native destination must remain a candidate through choose");
    assert_eq!(
        routed.chosen().id(),
        "native",
        "the deterministic fallback must be the one chosen when it is the only candidate"
    );
}

// ---------------------------------------------------------------------
// `GH-CAPABILITY-FACTS` — map lines 1517 and 1513, through the shipped
// binary rather than the library's public API. The tests above pin the
// two gates (`hard_constraint`'s `ToolSemantics::KnownAbsent` arm and
// `is_adequate`) against a hand-built `Destination` — proof the gate is
// correct on an input shape nothing in production could construct before
// this package (`docs/product/evidence/phase-35a.md`'s 1517/1513
// re-opening). These tests are the producer half: a real fixture config,
// read by `EffectiveConfig`, reaching `glasshouse route`'s own rejected
// section through `main.rs::routing_destinations`.
// ---------------------------------------------------------------------

mod shipped_binary {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const CREDENTIAL_VAR: &str = "GLASSHOUSE_CAPFACTS_TEST_KEY";

    /// A task the heuristic classifier reads as needing a shell — Phase 34's
    /// `SHELL_KEYWORDS` matches "run " and "shell" — which gives it a
    /// non-empty `hard_capabilities` (`HardCapability::ShellExecution`) and
    /// therefore `needs_tool_calls = true`
    /// (`routing::request::RouterRequest`'s own derivation): one task text
    /// drives both gates this package wires, exactly as `capability_fit`'s
    /// `axis_for` and `hard_constraint`'s tool-semantics check are two
    /// independent readings of the same classified requirement.
    const SHELL_TASK: &str = "run the test suite in the shell";

    struct Fixture {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new(config_toml: &str) -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let base = tmp.path().to_path_buf();
            let root = base.join("workspace");
            std::fs::create_dir_all(root.join(".git")).expect("create project root");
            let root = std::fs::canonicalize(&root).expect("canonicalize project root");

            let bin_dir = base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let harness = install_fake_harness(&bin_dir);
            let escaped = harness.display().to_string().replace('\\', "\\\\");

            let config_dir = base.join("config");
            std::fs::create_dir_all(&config_dir).expect("create config dir");
            std::fs::write(
                config_dir.join("config.toml"),
                config_toml.replace("{harness}", &escaped),
            )
            .expect("write user config");

            Self {
                _tmp: tmp,
                base,
                root,
            }
        }

        fn stdout(&self, args: &[&str]) -> String {
            let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(&self.root)
                .arg("--data-dir")
                .arg(self.base.join("data"))
                .arg("--config-dir")
                .arg(self.base.join("config"))
                .args(args)
                .env(CREDENTIAL_VAR, "planted-opaque-capfacts-value")
                .env("PATH", self.base.join("empty-path"))
                .output()
                .expect("the glasshouse binary must be runnable");
            assert!(
                output.status.success(),
                "glasshouse {args:?} must succeed:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        /// The `rejected` section of a `glasshouse route` report, or "" when
        /// nothing was rejected — mirrors `route_command.rs`'s own split.
        fn rejected(&self, task: &str) -> String {
            let report = self.stdout(&["route", "--task", task]);
            report
                .split_once("\nrejected\n")
                .map(|(_, rejected)| rejected.to_owned())
                .unwrap_or_default()
        }
    }

    /// Never actually invoked — `glasshouse route` decides and starts
    /// nothing — but `launch_profile` needs a resolvable path to build a
    /// destination from a configured profile at all.
    fn install_fake_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join(if cfg!(windows) {
            "fake-claude-code.cmd"
        } else {
            "fake-claude-code"
        });
        std::fs::write(
            &path,
            if cfg!(windows) {
                "@echo off\r\nexit /b 0\r\n"
            } else {
                "#!/bin/sh\nexit 0\n"
            },
        )
        .expect("write fake harness");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    /// REQUIRED BEHAVIOR bullet 1: `tool_calls = false` on the configured
    /// provider excludes its destination when the task needs tool calls,
    /// naming the tool-semantics constraint — `ProtocolSupport.tool_calls`
    /// reaching `Declared::verified(false, ..)` through
    /// `EffectiveConfig::configured_provider` and into `pairing.rs`'s
    /// `tool_semantics`, `ToolSemantics::KnownAbsent`, and
    /// `hard_constraint`'s tool-semantics arm — the whole chain the census
    /// (`docs/product/evidence/phase-35a.md`) found had no producer.
    #[test]
    fn a_declared_tool_calls_false_excludes_the_destination_1517() {
        let fixture = Fixture::new(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{harness}\"\n\n\
             [providers.route-probe]\ntemplate = \"openrouter\"\n\
             credential_env = [\"GLASSHOUSE_CAPFACTS_TEST_KEY\"]\n\
             tool_calls = false\n\n\
             [profiles.direct]\nharness = \"claude-code\"\n\
             expected_protocol = \"openai-chat\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n",
        );

        let rejected = fixture.rejected(SHELL_TASK);
        assert!(
            rejected.contains("fresh:claude-code:direct"),
            "a provider declared `tool_calls = false` must exclude the destination it backs \
             when the task needs tool calls:\n{rejected}"
        );
        assert!(
            rejected.contains("hard tool semantics constraint"),
            "the exclusion must be attributed to the tool-semantics constraint specifically, \
             not merely rejected for some other reason:\n{rejected}"
        );
    }

    /// REQUIRED BEHAVIOR bullet 1's other half: the identical fixture with
    /// `tool_calls` simply absent from the config ranks the destination
    /// exactly as it always has — `ProtocolSupport.tool_calls` stays the
    /// template's own `Unverified`, which `ToolSemantics::Unverified` never
    /// excludes on (`hard_constraint` only refuses `KnownAbsent`).
    #[test]
    fn tool_calls_absent_from_configuration_ranks_exactly_as_before_the_producer_existed() {
        let fixture = Fixture::new(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{harness}\"\n\n\
             [providers.route-probe]\ntemplate = \"openrouter\"\n\
             credential_env = [\"GLASSHOUSE_CAPFACTS_TEST_KEY\"]\n\n\
             [profiles.direct]\nharness = \"claude-code\"\n\
             expected_protocol = \"openai-chat\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n",
        );

        let rejected = fixture.rejected(SHELL_TASK);
        assert!(
            !rejected.contains("fresh:claude-code:direct"),
            "with no `tool_calls` declared, the destination must not be excluded on tool \
             semantics — an undeclared provider stays Unverified, never KnownAbsent:\n{rejected}"
        );
    }

    /// REQUIRED BEHAVIOR bullet 2: a model-scoped `shell_tool_use = false`
    /// excludes that model's destination when the task needs shell
    /// execution, through `EffectiveConfig::model_facts` →
    /// `Destination::with_resource_facts` → `ResourceCapabilities::describe`
    /// (whose `prefer` lets a `Verified` model fact override the harness's
    /// own declaration) → `is_adequate`.
    #[test]
    fn a_declared_shell_tool_use_false_excludes_that_models_destination() {
        let fixture = Fixture::new(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{harness}\"\n\n\
             [providers.route-probe]\ntemplate = \"openrouter\"\n\
             credential_env = [\"GLASSHOUSE_CAPFACTS_TEST_KEY\"]\n\n\
             [providers.route-probe.model_facts.shell-blind-model]\n\
             shell_tool_use = false\n\n\
             [profiles.direct]\nharness = \"claude-code\"\n\
             expected_protocol = \"openai-chat\"\n\
             model = \"shell-blind-model\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n",
        );

        let rejected = fixture.rejected(SHELL_TASK);
        assert!(
            rejected.contains("fresh:claude-code:direct"),
            "a model declared `shell_tool_use = false` must exclude the destination it backs \
             when the task needs shell execution:\n{rejected}"
        );
        assert!(
            rejected.contains("hard capability constraint"),
            "the exclusion must be attributed to the capability constraint specifically:\n\
             {rejected}"
        );
    }

    /// REQUIRED BEHAVIOR bullet 2's other half: a model this provider names
    /// no facts for stays `ResourceFacts::UNVERIFIED` and is never excluded
    /// — the same fixture, a different model name.
    #[test]
    fn an_undeclared_model_stays_unverified_and_is_never_excluded() {
        let fixture = Fixture::new(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{harness}\"\n\n\
             [providers.route-probe]\ntemplate = \"openrouter\"\n\
             credential_env = [\"GLASSHOUSE_CAPFACTS_TEST_KEY\"]\n\n\
             [providers.route-probe.model_facts.shell-blind-model]\n\
             shell_tool_use = false\n\n\
             [profiles.direct]\nharness = \"claude-code\"\n\
             expected_protocol = \"openai-chat\"\n\
             model = \"a-different-model\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n",
        );

        let rejected = fixture.rejected(SHELL_TASK);
        assert!(
            !rejected.contains("fresh:claude-code:direct"),
            "a model nobody declared facts for must stay Unverified and must not be \
             excluded, even though a sibling model on the same provider is declared \
             shell-blind:\n{rejected}"
        );
    }
}
