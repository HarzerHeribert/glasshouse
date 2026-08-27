//! Phase 9A lines 362, 363 and 366 — the generated-configuration mechanism of
//! the ephemeral child-process launch overlay.
//!
//! # What is tested where, and why
//!
//! Two levels, deliberately.
//!
//! The **binary** level covers what only a real process can answer: that
//! `glasshouse launch` really refuses a profile whose harness cannot be
//! pointed anywhere without a model, and that a refusal costs no session
//! record and no process. That is the §35 shape — the test enters through the
//! command a person types, and no helper in this file resolves a profile by
//! hand to make it convenient.
//!
//! The **library** level covers the document itself: where it lands, what is
//! in it, what the child is told, and that it does not outlive the guard. It
//! is at this level because the production caller that holds the guard is two
//! lines in `main.rs`, and `main.rs` was frozen for this package — see the
//! report. Every assertion here is about the exact sequence that patch
//! performs, in the same order: resolve, create the session's directory,
//! install into it, drop when the session ends.

use std::path::{Path, PathBuf};
use std::process::Command;

use glasshouse::harness::{GeneratedConfigSite, adapter_for};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile, Refusal, Resolution, resolve};
use glasshouse::provider::Provider;
use glasshouse::secret::{Secret, SecretRef, SecretStore};

/// The variable the binary-level fixture declares a credential under.
///
/// Only a *name* ever appears in this file. The claim that a value stays out
/// of a generated document is made in `profile/mod.rs`'s own tests, which can
/// mint a `Secret`; an integration test cannot, by design, and setting a real
/// environment variable to work around that would make the value visible to
/// everything else in the process — which is the exposure the mechanism
/// exists to avoid.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_LAUNCH_OVERLAY_KEY";

/// A [`SecretStore`] with nothing in it.
///
/// The library-level fixtures below declare no credential variable, so
/// nothing is ever asked for: those tests are about where a document lands
/// and how long it lives, and a credential would only be a second thing to
/// keep out of the assertions.
struct NoSecrets;

impl SecretStore for NoSecrets {
    fn resolve(&self, _reference: &SecretRef) -> Option<Secret> {
        None
    }
    fn is_present(&self, _reference: &SecretRef) -> bool {
        false
    }
    fn describe(&self) -> &'static str {
        "a store with nothing in it"
    }
}

/// A configured provider serving OpenAI chat completions at `base_url`.
fn probe_provider(base_url: &str) -> Provider {
    let mut provider = glasshouse::provider::templates()
        .iter()
        .find(|template| template.name == "openai-compatible")
        .cloned()
        .expect("the generic OpenAI-compatible template");
    provider.name = "probe-router".to_owned();
    for support in &mut provider.protocols {
        support.base_url = base_url.to_owned();
    }
    // No credential variable: see `NoSecrets`.
    provider.credential_env = Vec::new();
    provider
}

/// A launch profile pointing OpenCode at that provider.
fn opencode_profile(model: Option<&str>) -> LaunchProfile {
    let mut profile = LaunchProfile::native(IntegrationId::OpenCode);
    profile.name = "probe".to_owned();
    profile.backend = BackendResource::DirectProvider {
        provider: "probe-router".to_owned(),
    };
    profile.model = model.map(str::to_owned);
    profile
}

fn resolution<'a>(provider: &'a Provider, secrets: &'a dyn SecretStore) -> Resolution<'a> {
    Resolution {
        adapter: adapter_for(IntegrationId::OpenCode).expect("OpenCode has an adapter"),
        acknowledged_bypass: false,
        provider: Some(provider),
        secrets,
    }
}

/// The value of one environment key on an overlay.
fn env_value(overlay: &glasshouse::profile::LaunchOverlay, key: &str) -> Option<PathBuf> {
    overlay
        .env()
        .iter()
        .find(|(name, _)| name == std::ffi::OsStr::new(key))
        .map(|(_, value)| PathBuf::from(value))
}

/// The whole of line 362 in one observation: an isolated generated
/// configuration file, in the directory Glasshouse owns, that the child is
/// pointed at — and that does not survive the session.
///
/// This is the same sequence `main.rs::launch_session` performs, in the same
/// order, and each half of it is a separate claim:
///
/// - the document lands **inside the site**, and nowhere else on the machine;
/// - it is **owner-only** where the platform has a mode;
/// - the child's `OPENCODE_CONFIG` is **exactly the path that was written**,
///   not a similar one;
/// - and dropping the guard leaves nothing behind.
#[test]
fn a_generated_configuration_lives_in_the_directory_glasshouse_owns_and_dies_with_the_session() {
    let provider = probe_provider("https://probe.example/v1");
    let secrets = NoSecrets;
    let profile = opencode_profile(Some("probe-model-x"));

    let mut overlay = resolve(&profile, &resolution(&provider, &secrets))
        .expect("OpenCode can be pointed at an OpenAI-chat provider");

    // Resolution decides *what* the document is and never *where*: nothing
    // exists on disk yet, which is what keeps a refusal free.
    assert_eq!(
        overlay.configs().len(),
        1,
        "an OpenCode direct-provider profile needs exactly one generated document"
    );
    assert_eq!(overlay.configs()[0].file_name(), "opencode-provider.json");

    let tmp = tempfile::tempdir().expect("tempdir");
    // The session's own directory, which does not exist yet — exactly as it
    // does not when `launch_session` reaches this line.
    let session_dir = tmp.path().join("projects/p/sessions/abc123");
    assert!(!session_dir.exists());

    let installed = overlay
        .install(GeneratedConfigSite::new(&session_dir))
        .expect("the document must be writable into a directory Glasshouse owns");

    let written = installed.paths().to_vec();
    assert_eq!(written.len(), 1, "one document, one path");
    let path = &written[0];
    assert_eq!(
        path.parent(),
        Some(session_dir.as_path()),
        "a generated configuration may only land inside the site it was given: {}",
        path.display()
    );
    assert!(path.exists(), "the document must actually be on disk");

    // Nothing anywhere else under the temporary root, either — the check
    // that catches a document written beside the site rather than in it.
    let mut all = Vec::new();
    collect_files(tmp.path(), &mut all);
    assert_eq!(all, vec![path.clone()], "exactly one file was created");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a generated configuration is owner-only, not {:o}",
            mode & 0o777
        );
    }

    // The child is pointed at exactly this file.
    assert_eq!(
        env_value(&overlay, "OPENCODE_CONFIG").as_deref(),
        Some(path.as_path()),
        "the child must be pointed at the file that was written, not a similar one"
    );

    // And it is the *selection* that is missing without the document, not the
    // other way round: the argument pair is added during resolution, so an
    // overlay applied without being installed starts a harness that has been
    // told to use a provider it has never heard of.
    let args: Vec<String> = overlay
        .args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, vec!["--model", "probe-router/probe-model-x"]);

    // Ephemeral: the session ends, and so does the document.
    drop(installed);
    assert!(
        !path.exists(),
        "a generated configuration must not outlive the session that needed it"
    );
    let mut after = Vec::new();
    collect_files(tmp.path(), &mut after);
    assert!(
        after.is_empty(),
        "nothing generated may be left behind: {after:?}"
    );
}

/// A harness that needs no document generates none — the mechanism is
/// per-adapter, not a new thing every launch does.
///
/// Claude Code reaches a direct provider entirely through its environment, so
/// an installed overlay for one must create no file and no directory. The
/// assertion is on the directory not existing: a mechanism that wrote an
/// empty document, or created the session directory speculatively, would fail
/// here.
#[test]
fn a_harness_that_needs_no_generated_configuration_writes_nothing() {
    let mut provider = probe_provider("https://probe.example");
    provider.protocols[0].protocol = glasshouse::harness::WireProtocol::AnthropicMessages;
    let secrets = NoSecrets;

    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.name = "probe".to_owned();
    profile.backend = BackendResource::DirectProvider {
        provider: "probe-router".to_owned(),
    };

    let cx = Resolution {
        adapter: adapter_for(IntegrationId::ClaudeCode).expect("an adapter"),
        acknowledged_bypass: false,
        provider: Some(&provider),
        secrets: &secrets,
    };
    let mut overlay = resolve(&profile, &cx).expect("Claude Code takes a direct provider");
    assert!(
        overlay.configs().is_empty(),
        "Claude Code declares no generated configuration"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("session");
    let installed = overlay
        .install(GeneratedConfigSite::new(&site))
        .expect("installing nothing must succeed");
    assert!(installed.paths().is_empty());
    assert!(
        !site.exists(),
        "a harness that needs no document must not even have a directory made for it"
    );
}

/// A configured value may not smuggle a substitution into a document the
/// harness will interpolate before parsing.
///
/// OpenCode replaces `{env:NAME}` and `{file:PATH}` anywhere in a
/// configuration document. That is the mechanism Glasshouse uses to keep the
/// credential out of the file, and therefore a mechanism a base URL or header
/// value could use to put something else in.
#[test]
fn a_configured_value_cannot_smuggle_a_substitution_into_a_generated_document() {
    let secrets = NoSecrets;
    let profile = opencode_profile(Some("probe-model-x"));

    for (label, provider) in [
        (
            "base URL",
            probe_provider("https://probe.example/{env:HOME}/v1"),
        ),
        ("header value", {
            let mut provider = probe_provider("https://probe.example/v1");
            provider
                .headers
                .push(("X-Probe".to_owned(), "{file:/etc/passwd}".to_owned()));
            provider
        }),
    ] {
        let err = resolve(&profile, &resolution(&provider, &secrets))
            .expect_err("a substitution sequence in a configured value must be refused");
        assert!(
            matches!(err, Refusal::UnsafeGeneratedConfigValue { .. }),
            "{label}: wrong refusal: {err}"
        );
        // The refusal names the field and the sequence, never the value.
        let message = err.to_string();
        assert!(message.contains(label), "{label}: {message}");
        assert!(!message.contains("/etc/passwd"), "{label}: {message}");
    }
}

/// A refusal writes nothing, which is what makes "the file exists only for
/// this session" true of a session that never started.
#[test]
fn a_refused_profile_generates_no_document_at_all() {
    let provider = probe_provider("https://probe.example/v1");
    let secrets = NoSecrets;

    // No model, and OpenCode selects its provider through the model.
    let err = resolve(&opencode_profile(None), &resolution(&provider, &secrets))
        .expect_err("OpenCode cannot be pointed anywhere without a model");
    assert!(
        matches!(err, Refusal::DirectProviderNeedsModel { .. }),
        "{err}"
    );
    assert!(
        err.to_string()
            .contains("selects a provider through the model"),
        "the refusal must say why: {err}"
    );
}

/// Line 364, applied to the file mechanism: an unsupported combination is
/// refused rather than composed out of something plausible.
///
/// OpenCode was read sending `POST /v1/chat/completions` and nothing else, so
/// a provider that serves only Anthropic messages is a combination it has no
/// declaration for. Nothing is translated and no document is invented.
#[test]
fn an_unsupported_harness_and_protocol_combination_is_refused_rather_than_written() {
    let mut provider = probe_provider("https://probe.example/v1");
    provider.protocols[0].protocol = glasshouse::harness::WireProtocol::AnthropicMessages;
    let secrets = NoSecrets;

    let err = resolve(
        &opencode_profile(Some("probe-model-x")),
        &resolution(&provider, &secrets),
    )
    .expect_err("OpenCode does not speak anthropic-messages");
    assert!(
        matches!(err, Refusal::ProviderProtocolUnsupported { .. }),
        "{err}"
    );
}

// --- through the shipped binary --------------------------------------------

/// A project with its own data and config roots and a fake `opencode` on
/// `PATH`, so `session::select` resolves an installed harness without the
/// real one being present on the machine running the tests.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(profiles: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.opencode]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.probe-router]\ntemplate = \"openai-compatible\"\n\
                 base_url = \"https://probe.example/v1\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 {profiles}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-opencode");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-opencode.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// Through the command a person types: a profile whose harness cannot be
/// pointed anywhere without a model is refused, no process is started, and no
/// session is recorded.
///
/// This is the §35 half of the package. The refusal lives on the production
/// launch path rather than in a helper, and deleting the call to
/// `require_model_if_the_harness_selects_through_it` from
/// `apply_direct_provider` fails here — against the shipped binary, not a
/// fixture.
#[test]
fn the_binary_refuses_an_opencode_profile_that_names_no_model_and_records_no_session() {
    let fixture = Fixture::new(
        "[profiles.probe]\nharness = \"opencode\"\n\n\
         [profiles.probe.backend]\nkind = \"direct-provider\"\nprovider = \"probe-router\"\n",
    );

    let output = fixture.glasshouse(&["launch", "opencode", "--profile", "probe"]);
    assert!(
        !output.status.success(),
        "the launch must be refused:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("selects a provider through the model"),
        "the refusal must explain itself: {stderr}"
    );

    // A refusal costs nothing: no row, and nothing written under the state
    // directory for a session that never existed.
    let listing = fixture.glasshouse(&["sessions"]);
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(
        !listing.contains("opencode"),
        "a refused launch must record no session:\n{listing}"
    );
    let mut files = Vec::new();
    collect_files(&fixture.base.join("data"), &mut files);
    assert!(
        files
            .iter()
            .all(|path| path.file_name() != Some(std::ffi::OsStr::new("opencode-provider.json"))),
        "a refused launch must generate no configuration: {files:?}"
    );
}

/// Every regular file under `dir`, sorted, so a "nothing else was written"
/// assertion can name what it found.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
    out.sort();
}
