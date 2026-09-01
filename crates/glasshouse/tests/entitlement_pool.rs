//! Phase 56A lines 1962, 1963, 1964 and 1973, plus line 1947's job-kind
//! clause — the entitlement as the unit of capacity: a specific account with
//! its own credential, several per vendor, five explicit layers, and a
//! credential boundary between accounts.
//!
//! Two halves, for the reason `tests/entitlements.rs` gives for its own. The
//! first goes through the public configuration and routing API —
//! `EffectiveConfig::entitlements` / `entitlement_for` /
//! `entitlement_resources` and `DisposableRouting::choose` — entered exactly
//! as `main.rs` enters them. The second runs the shipped binary against
//! `[entitlements.<name>]` tables it wrote itself: nothing in half one can
//! fail on a build where `glasshouse status` stops listing configured
//! accounts, or where a launch's child environment carries the *other*
//! account's credential variable — and those are what this package wires.
//! Practice §35.
//!
//! Fixture credentials here are obviously fake strings; every artifact a
//! test can read (status output, launch stderr, a written config file, a
//! child's environment dump for the account NOT serving it) is asserted not
//! to contain the one that must not be there.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use glasshouse::RuntimePaths;
use glasshouse::config::{
    ConfigError, EffectiveConfig, EntitlementBacking, EntitlementCredential, EntitlementKind,
    EntitlementLookupError, EntitlementVendor, ProjectConfig, UserConfig,
    write_project_config_with_consent,
};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::BackendResource;
use glasshouse::project::Project;
use glasshouse::provider::registry::ResourceKind;
use glasshouse::routing::disposable::{
    DisposableCandidate, DisposableRouting, JobKind, NoResource,
};
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::{Cost, CredentialId, Entitlement, EntitlementRules};
use glasshouse::secret::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};

// ===========================================================================
// Half one — configuration: the entry is an account (1962, 1963, 1973a-d).
// ===========================================================================

/// Two accounts of one vendor and plan — map line 1963's own example — each
/// backed by its own provider entry and carrying its own credential
/// reference.
const TWO_CLAUDE_ACCOUNTS: &str = "version = 1\n\n\
     [providers.alpha-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_POOL_TEST_KEY_A\"]\n\n\
     [providers.beta-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_POOL_TEST_KEY_B\"]\n\n\
     [entitlements.claude-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
     provider = \"alpha-probe\"\ncredential = { env = \"GLASSHOUSE_POOL_TEST_KEY_A\" }\n\n\
     [entitlements.claude-b]\nkind = \"claude\"\nvendor = \"claude\"\n\
     provider = \"beta-probe\"\ncredential = { env = \"GLASSHOUSE_POOL_TEST_KEY_B\" }\n";

fn two_accounts() -> UserConfig {
    toml::from_str(TWO_CLAUDE_ACCOUNTS).expect("the two-account fixture parses")
}

/// **Line 1963.** Two entries of one vendor and one kind coexist: both are
/// resolved, both are named as distinct resources, each carries its own
/// credential reference, and each has its own remaining-capacity and
/// reset-time slots — `None`, because nothing has read them (56A package 2),
/// never full and never empty. Nothing dedupes by vendor.
#[test]
fn two_entitlements_of_one_vendor_and_kind_coexist_as_distinct_resources() {
    let user = two_accounts();
    let effective = EffectiveConfig::new(&user, None);

    let configured = effective
        .configured_entitlements()
        .expect("two accounts of one vendor are not a contradiction");
    assert_eq!(configured.len(), 2, "{configured:?}");
    let a = &configured[0];
    let b = &configured[1];
    assert_eq!(a.name(), "claude-a");
    assert_eq!(b.name(), "claude-b");
    assert_eq!(a.vendor(), Some(EntitlementVendor::Claude));
    assert_eq!(b.vendor(), Some(EntitlementVendor::Claude));
    assert_eq!(a.kind(), Some(EntitlementKind::Claude));
    assert_eq!(a.kind(), b.kind());
    assert_eq!(
        a.credential(),
        Some(&SecretRef::Environment {
            var: "GLASSHOUSE_POOL_TEST_KEY_A".to_owned()
        })
    );
    assert_eq!(
        b.credential(),
        Some(&SecretRef::Environment {
            var: "GLASSHOUSE_POOL_TEST_KEY_B".to_owned()
        })
    );
    for entry in [a, b] {
        assert!(
            entry.remaining_capacity().is_none(),
            "no telemetry reader exists in this build, so remaining capacity is unknown — \
             never a fabricated number: {entry:?}"
        );
        assert!(entry.seconds_until_reset().is_none());
    }

    // The registry shape: one resource per entry, keyed by name.
    let resources = effective
        .entitlement_resources()
        .expect("resolvable tables enumerate");
    assert_eq!(
        resources,
        vec![
            ResourceKind::Entitlement {
                name: "claude-a".to_owned()
            },
            ResourceKind::Entitlement {
                name: "claude-b".to_owned()
            },
        ],
        "two accounts of one vendor are two resources — nothing dedupes by vendor"
    );
}

/// **Line 1973 (a).** The credential field cannot express a value: only the
/// two reference shapes deserialise, and everything else is refused by a
/// sentence naming the rule. The refusals deliberately do not echo the
/// value-shaped text back (the custom messages here are ours; the TOML
/// parser's own line-and-column rendering is measured by
/// `the_refusal_message_this_crate_writes_never_contains_the_value` below).
#[test]
fn only_the_two_reference_shapes_deserialise_and_a_value_is_refused_by_name() {
    // The two shapes that must parse.
    let env: UserConfig = toml::from_str(
        "version = 1\n\n[entitlements.a]\ncredential = { env = \"POOL_ONLY_SHAPE_A\" }\n",
    )
    .expect("an environment reference parses");
    assert_eq!(
        env.entitlements()
            .get("a")
            .and_then(|e| e.credential())
            .map(EntitlementCredential::secret_ref),
        Some(&SecretRef::Environment {
            var: "POOL_ONLY_SHAPE_A".to_owned()
        })
    );
    let os: UserConfig = toml::from_str(
        "version = 1\n\n[entitlements.a]\n\
         credential = { service = \"glasshouse\", account = \"a\" }\n",
    )
    .expect("an OS-credential reference parses");
    assert_eq!(
        os.entitlements()
            .get("a")
            .and_then(|e| e.credential())
            .map(EntitlementCredential::secret_ref),
        Some(&SecretRef::OsCredential {
            service: "glasshouse".to_owned(),
            account: "a".to_owned()
        })
    );

    // A bare string — the value-shaped mistake — is refused by the rule's
    // name.
    let value = toml::from_str::<UserConfig>(
        "version = 1\n\n[entitlements.a]\n\
         credential = \"fake-not-a-real-key-0123456789\"\n",
    )
    .expect_err("a bare string must not deserialise");
    assert!(value.to_string().contains("never a value"), "{value}");

    // A map smuggling a value under another key is refused by that key's
    // name.
    let keyed = toml::from_str::<UserConfig>(
        "version = 1\n\n[entitlements.a]\n\
         credential = { value = \"fake-not-a-real-key-0123456789\" }\n",
    )
    .expect_err("a `value` key must not deserialise");
    assert!(
        keyed
            .to_string()
            .contains("does not take a key named `value`"),
        "{keyed}"
    );

    // The two shapes cannot be mixed, and half a shape is not a shape.
    for broken in [
        "version = 1\n\n[entitlements.a]\ncredential = { env = \"V\", service = \"s\" }\n",
        "version = 1\n\n[entitlements.a]\ncredential = { service = \"s\" }\n",
        "version = 1\n\n[entitlements.a]\ncredential = { account = \"a\" }\n",
    ] {
        toml::from_str::<UserConfig>(broken).expect_err(broken);
    }
}

/// A mistyped rule key is refused, not read as "no rule". The six rule
/// fields are plural and the singular is the typo a person actually makes;
/// an ignored key would leave `EntitlementRules::UNRESTRICTED`, which
/// *admits* the harness the line was written to keep out.
#[test]
fn a_mistyped_rule_key_is_refused_rather_than_read_as_no_rule() {
    for typo in [
        "deny_harness",
        "allow_harness",
        "deny_tier",
        "allow_tier",
        "deny_job_kind",
        "allow_job_kinds_",
    ] {
        let toml = format!("version = 1\n\n[entitlements.a]\n{typo} = [\"claude-code\"]\n");
        toml::from_str::<UserConfig>(&toml).expect_err(&toml);
    }
}

/// A value pasted where the `env` NAME belongs — the mistake one nesting
/// level deeper than a bare-string credential — is refused by shape, and
/// the refusal does not repeat the pasted value.
#[test]
fn an_env_value_shaped_like_a_credential_is_refused_without_being_echoed() {
    const PLANTED: &str = "sk-ant-api03-FAKEFAKE";
    let err = toml::from_str::<UserConfig>(&format!(
        "version = 1\n\n[entitlements.a]\ncredential = {{ env = \"{PLANTED}\" }}\n"
    ))
    .expect_err("a value pasted into `env` must not deserialise");
    assert!(
        err.message().contains("NAME"),
        "expected the env-is-not-a-value refusal, got: {}",
        err.message()
    );
    assert!(
        !err.message().contains(PLANTED),
        "the refusal repeated the pasted value: {}",
        err.message()
    );
}

/// The refusal *message* — the sentence this crate writes — never repeats
/// what was written. The TOML library's full rendering does quote the
/// offending config line under a caret (measured, not assumed: see this
/// package's report), so the one place a value-shaped credential is echoed
/// is the parser's own snippet of a file that already contained it — never
/// anything Glasshouse composes.
#[test]
fn the_refusal_message_this_crate_writes_never_contains_the_value() {
    const PLANTED: &str = "fake-planted-value-a1b2c3d4e5f6a1b2c3d4";
    let err = toml::from_str::<UserConfig>(&format!(
        "version = 1\n\n[entitlements.a]\ncredential = \"{PLANTED}\"\n"
    ))
    .expect_err("a bare string must not deserialise");
    assert!(err.message().contains("never a value"), "{}", err.message());
    assert!(
        !err.message().contains(PLANTED),
        "the refusal sentence repeated the value"
    );
}

/// A TOML parse error's rendering — what `main.rs` prints to stderr and
/// writes into `glasshouse.log` — passes through
/// [`glasshouse::secret::redact`] before it reaches this crate's `{err:#}`
/// chain. Without that, `toml`'s own `Display` quotes the whole offending
/// source line under a caret, so a pasted credential on the line that failed
/// to parse would be copied there verbatim.
#[test]
fn a_config_parse_failure_renders_with_the_credential_redacted() {
    const PLANTED: &str = "sk-ant-api03-FAKEFAKE-ON-THE-BROKEN-LINE";
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
    std::fs::create_dir_all(paths.config_dir()).expect("create config dir");
    std::fs::write(
        paths.user_config_file(),
        format!(
            "version = 1\n\n[entitlements.a]\ncredential = \"{PLANTED}\" this is not valid toml\n"
        ),
    )
    .expect("write malformed config");

    let err = UserConfig::load(&paths).expect_err("malformed TOML must not parse");
    assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
    let rendered = err.to_string();
    assert!(
        !rendered.contains(PLANTED),
        "the parse-error rendering repeated the planted credential: {rendered}"
    );
    assert!(
        rendered.contains(glasshouse::secret::REDACTED),
        "expected the redaction marker in: {rendered}"
    );
}

/// **Line 1973 (b).** `Debug` of every entitlement-carrying type renders
/// names only. The credential variable's *value* is planted in the real
/// process environment while formatting happens, so a `Debug` impl that
/// resolved the reference (the mutation this test exists to kill) would be
/// caught, not merely a `Debug` that happens to have nothing to print.
#[test]
fn debug_of_every_entitlement_type_never_contains_a_resolved_value() {
    const VAR: &str = "GLASSHOUSE_POOL_TEST_ONLY_DEBUG_VAR";
    const VALUE: &str = "fake-debug-value-9f8e7d6c5b4a-not-real";

    // SAFETY: `VAR` is unique to this test and removed again below, so no
    // other test can observe it set.
    unsafe {
        std::env::set_var(VAR, VALUE);
    }

    let credential = EntitlementCredential::environment(VAR);
    let mut config = glasshouse::config::EntitlementConfig::default();
    config
        .set_kind(Some(EntitlementKind::Claude))
        .set_vendor(Some(EntitlementVendor::Claude))
        .set_credential(Some(credential.clone()))
        .set_provider(Some("alpha-probe".to_owned()));
    let resolved = config
        .to_resolved("claude-a", glasshouse::config::Layer::User)
        .expect("a provider-backed entry with its own credential resolves");
    let routing = resolved.to_routing();

    let rendered = format!(
        "{credential:?}\n{config:?}\n{resolved:?}\n{routing:?}\n{}\n{}",
        resolved.describe(),
        glasshouse::routing::Entitlement::new("claude-a", EntitlementRules::UNRESTRICTED).name(),
    );

    unsafe {
        std::env::remove_var(VAR);
    }

    assert!(
        rendered.contains(VAR),
        "the reference's NAME is what a diagnostic is for: {rendered}"
    );
    assert!(
        !rendered.contains(VALUE),
        "a Debug or Display rendering resolved a credential value"
    );
}

/// **Line 1973 (c), the resolution half.** Each account's reference resolves
/// to its own value and never the other's, through the same store the
/// launch path uses. Compared without printing: even fabricated values do
/// not belong in test output.
#[test]
fn resolving_two_entitlements_yields_each_its_own_value_and_never_the_others() {
    const VAR_A: &str = "GLASSHOUSE_POOL_TEST_ONLY_RESOLVE_A";
    const VAR_B: &str = "GLASSHOUSE_POOL_TEST_ONLY_RESOLVE_B";
    const VALUE_A: &str = "fake-resolve-a-0123456789abcdef";
    const VALUE_B: &str = "fake-resolve-b-fedcba9876543210";

    let user: UserConfig = toml::from_str(&format!(
        "version = 1\n\n\
         [entitlements.claude-a]\nvendor = \"claude\"\ncredential = {{ env = \"{VAR_A}\" }}\n\n\
         [entitlements.claude-b]\nvendor = \"claude\"\ncredential = {{ env = \"{VAR_B}\" }}\n"
    ))
    .expect("two own-credential accounts parse");
    let effective = EffectiveConfig::new(&user, None);
    let configured = effective
        .configured_entitlements()
        .expect("two accounts resolve");
    let store = EnvironmentSecretStore::new();

    // SAFETY: both variables are unique to this test and removed before it
    // can panic, so no other test can observe them set.
    unsafe {
        std::env::set_var(VAR_A, VALUE_A);
        std::env::set_var(VAR_B, VALUE_B);
    }
    let resolved_a = configured[0].credential().and_then(|r| store.resolve(r));
    let resolved_b = configured[1].credential().and_then(|r| store.resolve(r));
    unsafe {
        std::env::remove_var(VAR_A);
        std::env::remove_var(VAR_B);
    }

    assert_eq!(configured[0].name(), "claude-a");
    assert_eq!(configured[1].name(), "claude-b");
    assert!(
        resolved_a.as_ref().map(Secret::expose) == Some(VALUE_A),
        "claude-a must resolve its own value"
    );
    assert!(
        resolved_b.as_ref().map(Secret::expose) == Some(VALUE_B),
        "claude-b must resolve its own value, never claude-a's"
    );
}

/// **Line 1973 (d).** The project-config writer serialises references only:
/// the written file names the variables and never the values behind them.
#[test]
fn the_project_config_writer_serialises_references_and_never_values() {
    const VAR_A: &str = "GLASSHOUSE_POOL_TEST_ONLY_WRITER_A";
    const VAR_B: &str = "GLASSHOUSE_POOL_TEST_ONLY_WRITER_B";
    const VALUE_A: &str = "fake-writer-a-0123456789abcdef";
    const VALUE_B: &str = "fake-writer-b-fedcba9876543210";

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("workspace");
    std::fs::create_dir_all(&root).expect("create project root");
    let project = Project::discover(&root, None, false).expect("a temp dir is a usable project");

    let mut config = ProjectConfig::default();
    let mut a = glasshouse::config::EntitlementConfig::default();
    a.set_vendor(Some(EntitlementVendor::Claude))
        .set_credential(Some(EntitlementCredential::environment(VAR_A)));
    config.entitlements_mut().set("claude-a", a);
    let mut b = glasshouse::config::EntitlementConfig::default();
    b.set_vendor(Some(EntitlementVendor::Claude))
        .set_credential(Some(EntitlementCredential::os_credential(
            "glasshouse",
            VAR_B,
        )));
    config.entitlements_mut().set("claude-b", b);

    // SAFETY: unique to this test; removed below. The values are in the
    // environment while the writer runs, so a writer that resolved a
    // reference would be caught rather than excused by an empty variable.
    unsafe {
        std::env::set_var(VAR_A, VALUE_A);
        std::env::set_var(VAR_B, VALUE_B);
    }
    write_project_config_with_consent(&project, &config).expect("the writer writes");
    unsafe {
        std::env::remove_var(VAR_A);
        std::env::remove_var(VAR_B);
    }

    let written = std::fs::read_to_string(root.join(".glasshouse/config.toml"))
        .expect("the project config was written");
    assert!(written.contains("[entitlements.claude-a]"), "{written}");
    assert!(written.contains(VAR_A), "{written}");
    assert!(written.contains(VAR_B), "{written}");
    assert!(
        !written.contains(VALUE_A) && !written.contains(VALUE_B),
        "a credential value reached a configuration file:\n{written}"
    );
}

/// **Line 1973's structural half in resolution.** One credential is one
/// account: two entries naming the same reference are refused by name, and
/// an entry claiming to be a harness's own sign-in while naming its own
/// credential is refused by name. The refusal messages carry the
/// references' names and nothing else.
#[test]
fn shared_credentials_and_native_sign_ins_with_credentials_are_refused_by_name() {
    let shared: UserConfig = toml::from_str(
        "version = 1\n\n\
         [entitlements.claude-a]\ncredential = { env = \"POOL_SHARED_VAR\" }\n\n\
         [entitlements.claude-b]\ncredential = { env = \"POOL_SHARED_VAR\" }\n",
    )
    .expect("parses; the contradiction is a resolution fact");
    let err = EffectiveConfig::new(&shared, None)
        .entitlements()
        .expect_err("one reference under two names is two names on one account");
    assert!(
        matches!(
            &err,
            EntitlementLookupError::SharedCredential { names, .. }
                if names == &["claude-a".to_owned(), "claude-b".to_owned()]
        ),
        "{err}"
    );
    assert!(err.to_string().contains("POOL_SHARED_VAR"), "{err}");

    let native: UserConfig = toml::from_str(
        "version = 1\n\n[entitlements.max]\nnative_harness = \"claude-code\"\n\
         credential = { env = \"POOL_NATIVE_VAR\" }\n",
    )
    .expect("parses; the contradiction is a resolution fact");
    let err = EffectiveConfig::new(&native, None)
        .entitlements()
        .expect_err("a harness's own sign-in has no credential of Glasshouse's to carry");
    assert!(
        matches!(&err, EntitlementLookupError::NativeSignInWithOwnCredential { name } if name == "max"),
        "{err}"
    );
}

// --- line 1964: five layers, separately replaceable -------------------------

/// **Line 1964, layer 1 (harness) alone.** The same entitlement serves two
/// harnesses: the lookup keys on the account's backing, not on who consumes
/// it, so nothing else moves when the harness does.
#[test]
fn the_same_entitlement_serves_two_harnesses() {
    let user = two_accounts();
    let effective = EffectiveConfig::new(&user, None);
    let backend = BackendResource::DirectProvider {
        provider: "alpha-probe".to_owned(),
    };

    let under_claude_code = effective
        .entitlement_for(IntegrationId::ClaudeCode, &backend)
        .expect("resolvable")
        .expect("claude-a names alpha-probe");
    let under_codex = effective
        .entitlement_for(IntegrationId::Codex, &backend)
        .expect("resolvable")
        .expect("claude-a names alpha-probe");
    assert_eq!(under_claude_code.name(), "claude-a");
    assert_eq!(under_codex.name(), "claude-a");
    assert_eq!(
        under_claude_code.credential(),
        under_codex.credential(),
        "the harness varied; authentication did not"
    );
}

/// **Line 1964, layer 4 (entitlement) alone.** The same harness runs under
/// two entitlements — one axis moves, the account changes, the harness does
/// not.
#[test]
fn the_same_harness_runs_under_two_entitlements() {
    let user = two_accounts();
    let effective = EffectiveConfig::new(&user, None);

    let under_a = effective
        .entitlement_for(
            IntegrationId::ClaudeCode,
            &BackendResource::DirectProvider {
                provider: "alpha-probe".to_owned(),
            },
        )
        .expect("resolvable")
        .expect("named");
    let under_b = effective
        .entitlement_for(
            IntegrationId::ClaudeCode,
            &BackendResource::DirectProvider {
                provider: "beta-probe".to_owned(),
            },
        )
        .expect("resolvable")
        .expect("named");
    assert_eq!(under_a.name(), "claude-a");
    assert_eq!(under_b.name(), "claude-b");
    assert_ne!(
        under_a.credential(),
        under_b.credential(),
        "two accounts, two authentications"
    );
}

/// **Line 1964, layer 5 (inference model) alone.** The same entitlement
/// serves two models: the model lives on the launch profile, and the
/// entitlement lookup cannot see it — varying it moves nothing else.
#[test]
fn the_same_entitlement_serves_two_models() {
    let user = two_accounts();
    let effective = EffectiveConfig::new(&user, None);

    let mut on_sonnet = glasshouse::profile::LaunchProfile::native(IntegrationId::ClaudeCode);
    on_sonnet.backend = BackendResource::DirectProvider {
        provider: "alpha-probe".to_owned(),
    };
    on_sonnet.model = Some("a-small-model".to_owned());
    let mut on_opus = on_sonnet.clone();
    on_opus.model = Some("a-large-model".to_owned());

    let for_sonnet = effective
        .entitlement_for(on_sonnet.harness, &on_sonnet.backend)
        .expect("resolvable")
        .expect("named");
    let for_opus = effective
        .entitlement_for(on_opus.harness, &on_opus.backend)
        .expect("resolvable")
        .expect("named");
    assert_ne!(on_sonnet.model, on_opus.model, "the model really varied");
    assert_eq!(for_sonnet.name(), for_opus.name());
    assert_eq!(for_sonnet.credential(), for_opus.credential());
}

/// **Line 1964, layer 3 (authentication) alone.** One vendor and one wire
/// protocol under two credentials: `claude-a` and `claude-b` share vendor,
/// kind, and the protocol adapter (both backings name the same provider
/// template), and differ in the credential reference and in nothing else
/// but their names.
#[test]
fn one_vendor_and_protocol_stand_behind_two_credentials() {
    let user = two_accounts();
    let effective = EffectiveConfig::new(&user, None);
    let configured = effective.configured_entitlements().expect("resolvable");
    let (a, b) = (&configured[0], &configured[1]);

    assert_eq!(a.vendor(), b.vendor(), "one vendor");
    assert_eq!(a.kind(), b.kind(), "one plan kind");
    // Both backings name providers cut from one template, so the protocol
    // adapter — the template's declared wire protocols — is identical by
    // construction; the entitlement entries themselves never name one.
    assert_eq!(
        user.providers().get("alpha-probe").map(|p| p.template()),
        user.providers().get("beta-probe").map(|p| p.template()),
        "one protocol adapter behind both accounts"
    );
    assert_ne!(a.credential(), b.credential(), "two authentications");
    assert!(
        matches!(a.backing(), EntitlementBacking::Provider(p) if p == "alpha-probe"),
        "{a:?}"
    );
}

// --- line 1947's job-kind clause: the disposable router consumes the rule --

fn free_candidate(provider: &str, model: &str) -> DisposableCandidate {
    DisposableCandidate::new(
        provider,
        model,
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_KEY", provider.to_uppercase().replace('-', "_")),
            },
        ),
        Cost::Free,
    )
}

/// **The rule reaches the decision.** A candidate whose entitlement denies
/// this job kind is never chosen — in either order, so the walk order cannot
/// be what decided it — and the choice's own explanation names the
/// entitlement and the job kind exactly as the session router's rejection
/// names an entitlement and a harness.
#[test]
fn an_entitlement_that_denies_the_job_kind_is_not_a_candidate_and_is_named() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let no_eval =
        free_candidate("alpha-probe", "model-one").with_entitlement(Some(Entitlement::new(
            "no-eval",
            EntitlementRules::UNRESTRICTED.deny_job_kinds([JobKind::Evaluation]),
        )));
    let open = free_candidate("beta-probe", "model-two").with_entitlement(Some(Entitlement::new(
        "open-account",
        EntitlementRules::UNRESTRICTED,
    )));

    for candidates in [
        vec![no_eval.clone(), open.clone()],
        vec![open.clone(), no_eval.clone()],
    ] {
        let choice = routing
            .choose(
                JobKind::Evaluation,
                &candidates,
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .expect("the open account serves");
        assert_eq!(choice.provider(), "beta-probe");
        let explanation = choice.explanation().render();
        assert!(
            explanation.contains("entitlement `no-eval` does not serve the `evaluation` job kind"),
            "the refusal names the entitlement and the job kind:\n{explanation}"
        );
    }

    // The same two candidates, a job kind nobody denies: the first free
    // candidate in the user's order wins exactly as before, and no refusal
    // is invented for the explanation.
    let choice = routing
        .choose(
            JobKind::Classification,
            &[no_eval, open],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("both serve classification");
    assert_eq!(choice.provider(), "alpha-probe");
    assert!(
        !choice.explanation().render().contains("entitlement rule"),
        "{}",
        choice.explanation().render()
    );
}

/// **A stated allow-list admits only its members** — the other way a rule
/// denies a job kind — and a candidate with no entitlement is never refused
/// by one: nobody's rule can refuse what nobody's rule describes.
#[test]
fn an_allow_list_omitting_the_job_kind_refuses_it_and_no_entitlement_never_does() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let classify_only =
        free_candidate("alpha-probe", "model-one").with_entitlement(Some(Entitlement::new(
            "classify-only",
            EntitlementRules::UNRESTRICTED.allow_job_kinds([JobKind::Classification]),
        )));
    let unnamed = free_candidate("beta-probe", "model-two");

    let choice = routing
        .choose(
            JobKind::MemoryExtraction,
            &[classify_only, unnamed],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("the unnamed candidate serves");
    assert_eq!(choice.provider(), "beta-probe");
    assert!(
        choice.explanation().render().contains(
            "entitlement `classify-only` does not serve the `memory extraction` job kind"
        ),
        "the spelling is JobKind::as_str's own, the one a rule is written in: {}",
        choice.explanation().render()
    );
}

/// **When every candidate's entitlement refuses, the error says so** —
/// naming each entitlement and the job kind — rather than misreporting the
/// pool as exhausted, which would send the user chasing quota instead of
/// their own rule.
#[test]
fn every_candidate_refused_names_every_entitlement_and_the_job_kind() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    let only = free_candidate("alpha-probe", "model-one").with_entitlement(Some(Entitlement::new(
        "no-eval",
        EntitlementRules::UNRESTRICTED.deny_job_kinds([JobKind::Evaluation]),
    )));

    let err = routing
        .choose(
            JobKind::Evaluation,
            &[only],
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect_err("nothing is left to serve");
    let NoResource::EntitlementDeniesEveryCandidate { reasons } = &err else {
        panic!("the refusal must name the rule, not the pool: {err:?}");
    };
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0].contains("entitlement `no-eval` does not serve the `evaluation` job kind"),
        "{reasons:?}"
    );
    assert!(err.to_string().contains("refuses this job kind"), "{err}");
}

// ===========================================================================
// Half two — the shipped binary: `glasshouse status` lists the pool, and a
// launch's child environment carries only the serving account's variable.
//
// The fixture is `tests/entitlements.rs`'s, reproduced rather than shared
// because integration tests are separate crates; the fake harness here dumps
// its environment as well as its argv, because the child's environment is
// exactly what line 1973 (c) is about.
// ===========================================================================

const VAR_A: &str = "GLASSHOUSE_POOL_BIN_KEY_A";
const VAR_B: &str = "GLASSHOUSE_POOL_BIN_KEY_B";
const VALUE_A: &str = "fake-pool-a-credential-0123456789abcdef";
const VALUE_B: &str = "fake-pool-b-credential-fedcba9876543210";

/// Two direct-provider launch profiles on two providers, and the two
/// accounts behind them — one per provider, one credential each.
fn pool_config() -> String {
    format!(
        "\n\
         [providers.alpha-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_A}\"]\n\n\
         [providers.beta-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{VAR_B}\"]\n\n\
         [profiles.alpha]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.alpha.backend]\nkind = \"direct-provider\"\n\
         provider = \"alpha-probe\"\n\n\
         [profiles.beta]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.beta.backend]\nkind = \"direct-provider\"\n\
         provider = \"beta-probe\"\n\n\
         [entitlements.claude-a]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"alpha-probe\"\ncredential = {{ env = \"{VAR_A}\" }}\n\n\
         [entitlements.claude-b]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"beta-probe\"\ncredential = {{ env = \"{VAR_B}\" }}\n"
    )
}

struct Binary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
    env_log: PathBuf,
}

impl Binary {
    fn with_config(extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let env_log = base.join("env.log");
        let harness = install_fake_harness(&bin_dir, &argv_log, &env_log);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\
                 {extra}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            argv_log,
            env_log,
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
            .env(VAR_A, VALUE_A)
            .env(VAR_B, VALUE_B)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.argv_log) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The child's own environment, as the fake harness dumped it.
    fn child_environment(&self) -> String {
        std::fs::read_to_string(&self.env_log).expect("the fake harness dumped its environment")
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, argv_log: &Path, env_log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-code");
    // `export -p` is a shell builtin, so the empty PATH the fixture launches
    // under cannot break it.
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexport -p > '{}'\nexit 0\n",
            argv_log.display(),
            env_log.display()
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, argv_log: &Path, env_log: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\necho %*>>\"{}\"\r\nset > \"{}\"\r\nexit /b 0\r\n",
            argv_log.display(),
            env_log.display()
        ),
    )
    .expect("write fake harness");
    path
}

/// **Line 1963 on the shipped binary.** `glasshouse status` lists both
/// accounts of the one vendor, by name — the enumeration is per entry, and
/// nothing between the configuration file and the status line dedupes by
/// vendor. This is the test that kills a registry deduplicating by vendor:
/// `claude-b` would vanish from this line.
#[test]
fn status_lists_both_accounts_of_one_vendor_by_name() {
    let binary = Binary::with_config(&pool_config());
    let out = binary.glasshouse(&["status"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("Entitlements 2 configured"),
        "the pool is announced:\n{said}"
    );
    assert!(
        said.contains("`claude-a`") && said.contains("`claude-b`"),
        "both accounts of one vendor are listed:\n{said}"
    );
    assert!(
        !said.contains(VALUE_A) && !said.contains(VALUE_B),
        "status carries names, never values:\n{said}"
    );
}

/// **Line 1973 (c), through the launch's environment builder and the real
/// child.** A launch under `claude-a` puts only `claude-a`'s variable into
/// the child environment: the serving credential arrives (the overlay's
/// doing), and the *other* account's variable — which the child would have
/// inherited from this very process — is scrubbed. Environment inheritance
/// is the real leak path, so the assertion is over the child's own dump of
/// its environment, not over what Glasshouse intended.
#[test]
fn a_launch_under_one_entitlement_never_carries_the_other_accounts_variable() {
    let binary = Binary::with_config(&pool_config());

    let out = binary.glasshouse(&["launch", "claude-code", "--headless", "--profile", "alpha"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "the launch must succeed:\n{said}");
    assert!(
        said.contains("entitlement `claude-a`"),
        "the launch says which account serves it:\n{said}"
    );
    assert_eq!(binary.harness_invocations().len(), 1);

    let child_env = binary.child_environment();
    assert!(
        child_env.contains(VALUE_A),
        "the serving account's credential must reach its own launch:\n{child_env}"
    );
    assert!(
        !child_env.contains(VAR_B) && !child_env.contains(VALUE_B),
        "the other account's variable was inherited into a session it does not serve:\n\
         {child_env}"
    );
    // And the streams the person reads never carry either value.
    assert!(!said.contains(VALUE_A) && !said.contains(VALUE_B), "{said}");
}

/// **The scrub's other direction.** A session no entitlement describes — a
/// plain native launch — carries *neither* account's variable: a session
/// charged to no account has no business holding any account's key.
///
/// Since map line 372 closed (2026-09-01), an unpinned launch under
/// automatic routing — the default — ranks every enabled profile
/// destination, the per-account ones included, so the ranking may
/// legitimately land on an account and carry that account's credential into
/// the launch it now serves (the previous test proves exactly that scrub).
/// This test's premise is the *other* path — a session no entitlement
/// serves — so it turns automatic routing off (map line 1712's own switch),
/// which keeps the unpinned launch native.
#[test]
fn a_launch_no_entitlement_serves_carries_no_accounts_variable() {
    let binary = Binary::with_config(&format!(
        "{}\n[routing]\nautomatic = false\n",
        pool_config()
    ));

    let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert_eq!(binary.harness_invocations().len(), 1);

    let child_env = binary.child_environment();
    assert!(
        !child_env.contains(VAR_A)
            && !child_env.contains(VALUE_A)
            && !child_env.contains(VAR_B)
            && !child_env.contains(VALUE_B),
        "a native session inherited a configured account's credential:\n{child_env}"
    );
}

/// **The production attach, through the shipped binary — practice §35.** The
/// half-one tests build their own candidates, so none of them would notice
/// `main.rs::disposable_candidates` ceasing to attach entitlements. This one
/// would: `glasshouse resources`' routing-model block reports what the real
/// automatic-classification decision — the same `disposable_candidates` +
/// `choose` path the shipped binary classifies with — would select, and with
/// the only configured candidate's entitlement denying `classification`, it
/// must report nothing to select and name the entitlement and the job kind.
#[test]
fn the_shipped_binary_attaches_entitlements_to_support_work_candidates() {
    let config = format!(
        "\n\
         [routing]\nmodel = {{ kind = \"automatic\" }}\n\n\
         [providers.alpha-probe]\ntemplate = \"openai-compatible\"\n\
         base_url = \"http://127.0.0.1:9/v1\"\n\
         credential_env = [\"{VAR_A}\"]\nfree_models = [\"a-free-model\"]\n\n\
         [entitlements.no-classify]\nvendor = \"openrouter\"\nprovider = \"alpha-probe\"\n\
         credential = {{ env = \"{VAR_A}\" }}\ndeny_job_kinds = [\"classification\"]\n"
    );
    let binary = Binary::with_config(&config);
    let out = binary.glasshouse(&["resources", "--no-harness"]);
    let said = Binary::both_streams(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("would select    nothing"),
        "an entitlement-refused candidate must not be selectable:\n{said}"
    );
    assert!(
        said.contains("entitlement `no-classify` does not serve the `classification` job kind"),
        "the refusal names the entitlement and the job kind, from the shipped binary's own \
         candidate list:\n{said}"
    );
    assert!(!said.contains(VALUE_A), "{said}");
}
