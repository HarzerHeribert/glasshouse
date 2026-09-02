//! Ten V1-completion criteria (map lines 1899–1908) over Glasshouse's
//! setup/portability phases the map already records closed — harness
//! detection, launch profiles, the local gateway, provider templates,
//! free-pool routing, and response profiles — proved against the shipped
//! binary or the nearest deterministic production seam, per
//! `.agent-runtime/packet-prove-it-54a.md`. Each test's doc comment quotes
//! the map line, names the evidence entry that proves the underlying
//! mechanism, and states its mutation.
//!
//! Unix only (`#![cfg(unix)]`): every fake harness here is a `#!/bin/sh`
//! script, the same shape `tests/v1_criteria_sessions.rs` and
//! `tests/pty_smoke.rs` use, and `chmod +x` only means something on unix.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;

use glasshouse::config::{ProviderConfig, RoutingModelChoice, UserConfig};
use glasshouse::gateway::{Route, Upstream, UpstreamBackend};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::response::floor_directive;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};
use glasshouse::{Cli, Runtime, bootstrap};

// ---------------------------------------------------------------------------
// Shared fixtures — trimmed from `tests/v1_criteria_sessions.rs` (harness
// installers, `run`, `field`) and `tests/gateway_degrade.rs` (raw HTTP over a
// gateway), reproduced here because both files' helpers are private to them.
// ---------------------------------------------------------------------------

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn git_project(base: &Path, name: &str) -> PathBuf {
    let root = base.join("workspace").join(name);
    std::fs::create_dir_all(root.join(".git")).expect("create project root");
    std::fs::canonicalize(&root).expect("canonicalize project root")
}

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

fn make_executable(path: &Path) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fake installed harness that exits immediately, touching nothing.
fn install_quiet_harness(bin_dir: &Path, name: &str) -> PathBuf {
    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write quiet harness");
    make_executable(&path);
    path
}

/// A fake installed harness that dumps its own argv and environment to
/// `dump.txt` under its working directory (the project root, per Phase 1)
/// before exiting — the same "write a side channel a headless launch cannot
/// otherwise observe" idiom `tests/v1_criteria_sessions.rs`'s pid-file
/// harness and `tests/pty_smoke.rs::install_tagged_echo_harness` both use.
fn install_dumping_harness(bin_dir: &Path, name: &str) -> PathBuf {
    let path = bin_dir.join(name);
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         echo \"ARGV:$*\" > \"$PWD/dump.txt\"\n\
         env >> \"$PWD/dump.txt\"\n\
         exit 0\n",
    )
    .expect("write dumping harness");
    make_executable(&path);
    path
}

fn run(data_dir: &Path, config_dir: &Path, root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(root)
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--config-dir")
        .arg(config_dir)
        .args(args)
        .output()
        .expect("the glasshouse binary must be runnable")
}

fn bootstrap_at(data_dir: &Path, config_dir: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--scope",
        root.to_str().unwrap(),
        "--data-dir",
        data_dir.to_str().unwrap(),
        "--config-dir",
        config_dir.to_str().unwrap(),
    ])
    .expect("parse the fixture command line");
    bootstrap(&cli, root).expect("bootstrap the fixture runtime")
}

fn all_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(all_files_under(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// One field's value from `glasshouse sessions show`'s
/// `{label:<19}{value}` layout (`main.rs::session_report`).
fn field<'a>(detail: &'a str, label: &str) -> &'a str {
    detail
        .lines()
        .find_map(|line| line.strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("no {label:?} line in:\n{detail}"))
}

/// The session ids `glasshouse sessions` lists, in listing order.
fn session_ids(data_dir: &Path, config_dir: &Path, root: &Path) -> Vec<String> {
    let listing = run(data_dir, config_dir, root, &["sessions"]);
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect()
}

fn messages_request(token: &str) -> Vec<u8> {
    let body = "{\"model\":\"probe\"}";
    format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Anthropic-Version: 2023-06-01\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

/// Send `raw` and return everything the peer wrote back, to the close.
fn send_and_read(address: SocketAddr, raw: &[u8]) -> String {
    let mut client = TcpStream::connect(address).expect("the peer accepts connections");
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a non-zero read timeout is valid");
    client.write_all(raw).expect("the peer reads the request");
    client.flush().expect("the peer reads the request");
    let mut out = Vec::new();
    client
        .read_to_end(&mut out)
        .or_else(|err| match err.kind() {
            std::io::ErrorKind::ConnectionReset => Ok(out.len()),
            _ => Err(err),
        })
        .expect("the peer answers and then closes");
    String::from_utf8_lossy(&out).into_owned()
}

/// A stand-in credential, resolved through the real environment secret store
/// rather than a crate-private constructor — the same idiom
/// `tests/gateway_degrade.rs::test_credential` uses, reproduced here because
/// that helper is private to its file.
fn test_credential(var: &str, value: &str) -> Secret {
    // SAFETY: `var` is unique to its one caller in this file and is removed
    // again immediately below, before the resolved value is even inspected.
    unsafe {
        std::env::set_var(var, value);
    }
    let resolved = EnvironmentSecretStore::new()
        .resolve(&SecretRef::Environment {
            var: var.to_owned(),
        })
        .expect("the variable was just set");
    unsafe {
        std::env::remove_var(var);
    }
    resolved
}

/// One request as recorded off the wire: method, target, lower-cased header
/// names with their values exactly as received, and the body. Parses
/// independently of `glasshouse::gateway::http` for the reason
/// `gateway::fixture`'s own module doc gives: a fixture that reused the
/// parser under test would agree with it about a request it had mis-framed.
#[derive(Debug, Clone)]
struct RecordedRequest {
    #[allow(dead_code)]
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A canned HTTP upstream on loopback, answering every request `200` with a
/// fixed body, and recording what it actually received.
struct FixtureUpstream {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
}

impl FixtureUpstream {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback must bind");
        listener.set_nonblocking(true).expect("polling mode");
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve_fixture(stream, &thread_requests);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            requests,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FixtureUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve_fixture(mut stream: TcpStream, requests: &Arc<Mutex<Vec<RecordedRequest>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();

    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);

    requests.lock().unwrap().push(RecordedRequest {
        method,
        target,
        headers,
    });

    let document = r#"{"data":[{"id":"fixture-model"}]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{document}",
        document.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// 1899 — zero-config detection
// ---------------------------------------------------------------------------

/// **1899** — "Consider onboarding usable when a new user can launch
/// Glasshouse and see installed supported harnesses without manually editing
/// a config file."
///
/// Shape: `tests/v1_criteria_sessions.rs`'s fresh-project pattern, entering
/// at `glasshouse doctor` (`main.rs::Command::Doctor` →
/// `integrations::doctor_report`), the production caller
/// `docs/product/evidence/phase-2b.md` records for harness detection.
/// Entry: `phase-2b.md` ("Mark every detected integration as available,
/// configured, unconfigured, unsupported-version, or unknown").
///
/// Mutation: `integrations/mod.rs::doctor_report`'s harness loop, replaced
/// with an empty iterator — "hide detected harnesses from the report a new
/// user actually reads."
#[test]
fn v1_1899_a_fresh_project_shows_installed_harnesses_with_no_config_file_created() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    // Deliberately NOT pre-created: the criterion is that no config file is
    // written, and `bootstrap`/`doctor` must not need one to exist first.
    let config_dir = base.join("config");
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let claude = install_quiet_harness(&bin_dir, "claude");
    let codex = install_quiet_harness(&bin_dir, "codex");

    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&project)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .arg("doctor")
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .expect("run glasshouse doctor");
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);

    for (label, path) in [("Claude Code", &claude), ("Codex", &codex)] {
        let row = report
            .lines()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no {label} row in doctor report:\n{report}"));
        assert!(
            row.contains(&path.display().to_string()),
            "{label}'s row must name the executable resolved from PATH: {row}"
        );
        assert!(
            !row.to_ascii_lowercase().contains("notfound"),
            "{label} was on PATH and must not be reported not found: {row}"
        );
    }

    assert!(
        !config_dir.join("config.toml").exists(),
        "doctor must never write the user config file just by running"
    );
    assert!(
        all_files_under(&config_dir).is_empty(),
        "doctor must create no file at all under the config directory: {:?}",
        all_files_under(&config_dir)
    );
}

// ---------------------------------------------------------------------------
// 1900 — native launch needs no provider
// ---------------------------------------------------------------------------

/// **1900** — "Consider onboarding usable when the user can skip all
/// provider configuration and still use native detected harnesses."
///
/// Shape: `tests/v1_criteria_sessions.rs::v1_1922`'s headless-launch pattern,
/// with no `[providers.*]` table anywhere in configuration. Entry:
/// `phase-9a.md` (native launch profiles).
///
/// Mutation: `profile/mod.rs::resolve`'s `BackendResource::Native` arm,
/// panicking instead of completing — the native resolution path this
/// launch's own `Runtime` sits directly on top of.
#[test]
fn v1_1900_a_native_launch_needs_no_provider_configuration() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_quiet_harness(&bin_dir, "quiet-claude-code");
    // Only the harness is declared. No `[providers.*]` table exists at all.
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let output = run(
        &data_dir,
        &config_dir,
        &project,
        &["launch", "claude-code", "--headless"],
    );
    assert!(
        output.status.success(),
        "a native launch with zero provider configuration must still work: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let ids = session_ids(&data_dir, &config_dir, &project);
    assert_eq!(ids.len(), 1, "expected one recorded session");
    let show = run(
        &data_dir,
        &config_dir,
        &project,
        &["sessions", "show", &ids[0]],
    );
    let detail = String::from_utf8_lossy(&show.stdout).into_owned();
    assert_ne!(field(&detail, "harness"), "-", "{detail}");
}

// ---------------------------------------------------------------------------
// 1901 — a provider added later is seen without rerunning setup
// ---------------------------------------------------------------------------

/// **1901** — "Consider settings usable when the user can return later and
/// configure a provider without rerunning the entire setup."
///
/// Shape: config edited directly on disk between two `glasshouse resources`
/// runs, onboarding never touched — `setup` is never invoked. Entry:
/// `phase-2d.md` (the Providers settings section), `phase-2c.md`
/// (onboarding persisted so the wizard runs at most once).
///
/// Mutation: `integrations/mod.rs`'s "Configured providers" section,
/// discarding `effective.provider_names()` in favour of an empty list — the
/// caller `doctor` actually reads, on the same read-fresh-every-run
/// `EffectiveConfig` the criterion is about.
#[test]
fn v1_1901_a_provider_added_later_is_seen_without_rerunning_setup() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Onboarding already completed — `setup` is never run again by this
    // test, and this stays true throughout.
    std::fs::write(
        config_dir.join("config.toml"),
        "version = 1\n\n[onboarding]\ncompleted = true\ncompleted_at_version = \"9.9.9\"\n",
    )
    .unwrap();

    let before = run(&data_dir, &config_dir, &project, &["doctor"]);
    assert!(before.status.success(), "{:?}", before);
    let before_text = String::from_utf8_lossy(&before.stdout);
    assert!(
        !before_text.contains("v1901-provider"),
        "the not-yet-configured provider must not appear yet:\n{before_text}"
    );

    // The user edits the file directly — no `glasshouse setup` in between.
    let mut config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    config.push_str(
        "\n[providers.v1901-provider]\ntemplate = \"openai-compatible\"\n\
         base_url = \"http://127.0.0.1:1\"\ncredential_env = [\"V1901_KEY\"]\n",
    );
    std::fs::write(config_dir.join("config.toml"), config).unwrap();

    let after = run(&data_dir, &config_dir, &project, &["doctor"]);
    assert!(after.status.success(), "{:?}", after);
    let after_text = String::from_utf8_lossy(&after.stdout);
    assert!(
        after_text.contains("v1901-provider"),
        "a provider added directly to the config file must be seen on the very next run, \
         with no setup rerun in between:\n{after_text}"
    );

    // Onboarding really was never rerun — still the version this test
    // planted, not whatever `setup` would have written.
    let final_config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(
        final_config.contains("completed_at_version = \"9.9.9\""),
        "onboarding must not have been touched by any of this:\n{final_config}"
    );
}

// ---------------------------------------------------------------------------
// 1902 — the same claude-code binary, native or alternate, never touches the
// harness's own configuration
// ---------------------------------------------------------------------------

/// **1902** — "Consider launch profiles usable when the same installed
/// Claude Code binary can be started natively or with an alternate
/// compatible provider without modifying the user's normal Claude
/// configuration."
///
/// Shape: `tests/v1_criteria_sessions.rs::v1_1917`'s "fake $HOME receives
/// nothing" pattern, applied to both a native and a direct-provider launch of
/// the same fake `claude` binary. Entry: `phase-9f.md` (direct provider
/// launch profiles: "the user's native harness authentication and global
/// configuration are never modified"; Claude Code carries the alternate
/// provider over three environment variables, never a file).
///
/// Mutation: no production code writes into `$HOME/.claude` on either
/// launch — the guarantee is an absence, so §17's "prove an absence
/// assertion in both directions" applies: the mutation is on the fake
/// harness fixture itself, made to write a sentinel into
/// `$HOME/.claude/settings.json`. KILLED confirms the byte-identical
/// assertion actually watches that path rather than passing vacuously.
#[test]
fn v1_1902_native_and_alternate_provider_launches_never_touch_claudes_own_config() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_home = base.join("fake-home");
    let claude_dir = fake_home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "{\"sentinel\":true}\n").unwrap();

    let before = all_files_under(&fake_home);
    let before_hashes: Vec<(PathBuf, Vec<u8>)> = before
        .iter()
        .map(|p| (p.clone(), std::fs::read(p).unwrap()))
        .collect();

    let harness = install_quiet_harness(&bin_dir, "quiet-claude-code");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [providers.v1902-probe]\ntemplate = \"anthropic-compatible\"\n\
             base_url = \"http://127.0.0.1:1\"\n\
             credential_env = [\"V1902_PROBE_KEY\"]\n\n\
             [profiles.direct]\nharness = \"claude-code\"\n\n\
             [profiles.direct.backend]\nkind = \"direct-provider\"\n\
             provider = \"v1902-probe\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let launch = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&project)
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--config-dir")
            .arg(&config_dir)
            .args(args)
            .env("HOME", &fake_home)
            .env("V1902_PROBE_KEY", "sk-planted-not-a-real-key-1902")
            .output()
            .expect("run glasshouse launch")
    };

    let native = launch(&["launch", "claude-code", "--headless"]);
    assert!(
        native.status.success(),
        "native launch failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let direct = launch(&["launch", "claude-code", "--headless", "--profile", "direct"]);
    assert!(
        direct.status.success(),
        "direct-provider launch failed: {}",
        String::from_utf8_lossy(&direct.stderr)
    );

    let after = all_files_under(&fake_home);
    let after_hashes: Vec<(PathBuf, Vec<u8>)> = after
        .iter()
        .map(|p| (p.clone(), std::fs::read(p).unwrap()))
        .collect();
    assert_eq!(
        before_hashes, after_hashes,
        "the fake $HOME/.claude directory must be byte-identical before and after both the \
         native and the alternate-provider launch"
    );
}

// ---------------------------------------------------------------------------
// 1903 — a gateway-backed session is operated only by the installed harness
// ---------------------------------------------------------------------------

/// **1903** — "Consider interactive gateway use valid only when the session
/// is operated by an installed compatible harness and Glasshouse does not
/// create a replacement agent loop."
///
/// The architectural half — Glasshouse spawns no agent loop of its own — is
/// established structurally, not by a test that could pass against a build
/// that added one: `phase-9g.md` records the local gateway as a pure HTTP
/// relay (`gateway::accept_loop`, `ingress::serve`) with no model call and no
/// conversation state of its own anywhere in the crate. This test supplies
/// the launch-shaped half the packet asks for: a gateway-backed launch is
/// nothing but the installed harness child, pointed at the gateway over
/// environment variables, with Glasshouse's own process never in the
/// request path.
///
/// Entry: `phase-9g.md` ("the local gateway process").
///
/// Mutation: `main.rs::launch_session`'s gateway-backend arm, skipping the
/// call to `gateway_upstream`/`start_if_required` and forwarding the native
/// profile's environment instead — "a gateway-backed launch composes no
/// gateway environment for its child" — expressed on the seam this test
/// actually reads: `harness/claude_code.rs`'s `ANTHROPIC_BASE_URL` variable
/// name.
#[test]
fn v1_1903_a_gateway_backed_launch_hands_the_session_to_the_installed_harness_alone() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_dumping_harness(&bin_dir, "dumping-claude-code");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [providers.v1903-probe]\ntemplate = \"anthropic-compatible\"\n\
             base_url = \"http://127.0.0.1:1\"\n\
             credential_env = [\"V1903_PROBE_KEY\"]\n\n\
             [profiles.gateway]\nharness = \"claude-code\"\n\n\
             [profiles.gateway.backend]\nkind = \"glasshouse-gateway\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&project)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .args([
            "launch",
            "claude-code",
            "--headless",
            "--profile",
            "gateway",
        ])
        .env("V1903_PROBE_KEY", "sk-planted-not-a-real-key-1903")
        .output()
        .expect("run glasshouse launch");
    assert!(
        output.status.success(),
        "gateway-backed launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dump = std::fs::read_to_string(project.join("dump.txt"))
        .expect("the harness must have run in the project root and left its dump");
    let base_url_line = dump
        .lines()
        .find(|line| line.starts_with("ANTHROPIC_BASE_URL="))
        .unwrap_or_else(|| panic!("no ANTHROPIC_BASE_URL in the child's environment:\n{dump}"));
    assert!(
        base_url_line.contains("127.0.0.1"),
        "a gateway-backed launch must point the harness at the local loopback gateway, not the \
         configured provider directly: {base_url_line}"
    );
    assert!(
        !base_url_line.contains(":1\""),
        "the harness must never be pointed at the provider's own (unreachable) base URL \
         directly: {base_url_line}"
    );

    let ids = session_ids(&data_dir, &config_dir, &project);
    assert_eq!(ids.len(), 1);
    let show = run(
        &data_dir,
        &config_dir,
        &project,
        &["sessions", "show", &ids[0]],
    );
    let detail = String::from_utf8_lossy(&show.stdout).into_owned();
    assert_eq!(
        field(&detail, "backend resource"),
        "glasshouse-gateway",
        "{detail}"
    );
}

// ---------------------------------------------------------------------------
// 1904 — a response profile reaches the harness through a native mechanism,
// coding instructions preserved
// ---------------------------------------------------------------------------

/// **1904** — "Consider response profiles minimally usable when at least one
/// supported harness can apply a selected profile through a native mechanism
/// or the bounded additive fallback while preserving coding instructions."
///
/// `tests/response_profiles.rs` already proves the composition at the seam
/// (`HarnessSelection::install_session_document`, which `main.rs`'s launch
/// path calls) — `the_launch_carries_exactly_one_settings_flag_and_keeps_the_lifecycle_hooks`
/// and `the_launch_appends_to_the_system_prompt_and_never_replaces_it`. This
/// test adds the launch-shaped half: the argv Claude Code's own native
/// `--settings` mechanism receives, read back from a real headless launch of
/// the shipped binary rather than from the composer's return value.
///
/// Entry: `phase-9k.md` ("Profile model, 11 of 11" — 587–597; `apply` asking
/// for a native mechanism first).
///
/// Mutation: `harness/claude_code.rs`'s `--settings` flag literal, changed to
/// a flag Claude Code does not recognise — "the native mechanism's own flag
/// name."
#[test]
fn v1_1904_a_launched_harness_receives_the_response_profile_through_its_native_settings_flag() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_dumping_harness(&bin_dir, "dumping-claude-code");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [response]\npreset = \"concise-technical\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    let output = run(
        &data_dir,
        &config_dir,
        &project,
        &["launch", "claude-code", "--headless"],
    );
    assert!(
        output.status.success(),
        "launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dump = std::fs::read_to_string(project.join("dump.txt"))
        .expect("the harness must have run and left its dump");
    let argv_line = dump
        .lines()
        .next()
        .unwrap_or_else(|| panic!("no ARGV line in dump:\n{dump}"));
    assert_eq!(
        argv_line.matches("--settings").count(),
        1,
        "the native mechanism must carry the profile through exactly one --settings flag, \
         never a second one that would silently discard the first: {argv_line}"
    );
    assert!(
        !argv_line.contains("--system-prompt"),
        "a response profile must never replace the coding system prompt outright: {argv_line}"
    );

    // The settings document itself, at the path `--settings` names in argv,
    // carries the profile's own value — not just an empty flag.
    let settings_path = argv_line
        .split("--settings")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("--settings carried no path: {argv_line}"));
    let settings = std::fs::read_to_string(settings_path.trim_matches('"'))
        .unwrap_or_else(|err| panic!("could not read {settings_path}: {err}"));
    assert!(
        settings.contains("outputStyle"),
        "the settings document must carry the resolved profile: {settings}"
    );
}

// ---------------------------------------------------------------------------
// 1924 — a response profile controls communication without reducing
// verification or replacing native harness coding instructions
// ---------------------------------------------------------------------------

/// **1924** — "Consider V1 usable when a response profile can control
/// user-facing communication without reducing verification or replacing
/// native harness coding instructions."
///
/// 1904's line restated as a V1 completion criterion with two extra clauses
/// this test proves against the same launch shape: (1) the selected preset's
/// native style actually reaches the settings document the `--settings` flag
/// names, (2) the floor sentence — [`floor_directive`], `REQUIRED_REPORTS` —
/// rides along unconditionally through `--append-system-prompt`, and (3)
/// neither the project's own coding-instructions file nor a stand-in for the
/// harness's own user-level config is touched by the launch.
///
/// Entry: `docs/product/evidence/phase-54a.md`'s 1904 entry — same production
/// seams, `src/harness/claude_code.rs` (`native_response_style`,
/// `additive_response_injection`, `closest_output_style`) and
/// `src/harness/response.rs::apply` (325–380).
///
/// Mutations:
/// - M1 (`src/harness/claude_code.rs`, `OUTPUT_STYLE_KEY`): rename the key
///   away from `"outputStyle"` — the composed settings document no longer
///   carries the key clause 1 reads.
/// - M2 (`src/harness/response.rs:369`): drop
///   `push(OsString::from(floor_directive()))` — clause 2's floor sentence
///   never reaches argv.
/// - M3 (`src/harness/claude_code.rs:305`): `"--append-system-prompt"` ->
///   `"--system-prompt"` — the profile would replace, not append.
#[test]
fn v1_1924_a_response_profile_controls_communication_without_reducing_verification_or_replacing_coding_instructions()
 {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let bin_dir = base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let harness = install_dumping_harness(&bin_dir, "dumping-claude-code");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
             [response]\npreset = \"concise-technical\"\n",
            toml_path(&harness)
        ),
    )
    .unwrap();

    // The project's own coding-instructions file, and a directory standing in
    // for the harness's own user-level config — neither is a Glasshouse path,
    // and clause 3 ("... or replacing native harness coding instructions")
    // requires the launch to leave both alone.
    let coding_instructions = project.join("CLAUDE.md");
    std::fs::write(
        &coding_instructions,
        "these are the project's own coding instructions\n",
    )
    .unwrap();
    let harness_home = base.join("harness-home");
    std::fs::create_dir_all(&harness_home).unwrap();
    let harness_home_marker = harness_home.join("settings.json");
    std::fs::write(
        &harness_home_marker,
        "{\"outputStyle\":\"whatever-the-user-set\"}\n",
    )
    .unwrap();

    let before_instructions = std::fs::read(&coding_instructions).unwrap();
    let before_harness_home = std::fs::read(&harness_home_marker).unwrap();

    let output = run(
        &data_dir,
        &config_dir,
        &project,
        &["launch", "claude-code", "--headless"],
    );
    assert!(
        output.status.success(),
        "launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Clause 3, half one: the project's coding instructions are
    // byte-identical before and after the launch.
    let after_instructions = std::fs::read(&coding_instructions).unwrap();
    assert_eq!(
        before_instructions, after_instructions,
        "a response profile must never touch the project's own coding instructions"
    );
    // Clause 3, half two: the harness's own user-level config is untouched —
    // Glasshouse's own settings document lives under its own data dir, never
    // here.
    let after_harness_home = std::fs::read(&harness_home_marker).unwrap();
    assert_eq!(
        before_harness_home, after_harness_home,
        "a response profile must never touch the harness's own user-level config"
    );

    let dump = std::fs::read_to_string(project.join("dump.txt"))
        .expect("the harness must have run and left its dump");
    let argv_line = dump
        .lines()
        .next()
        .unwrap_or_else(|| panic!("no ARGV line in dump:\n{dump}"));

    // Clause 3, half three: no bare `--system-prompt` in argv — that would
    // replace the coding system prompt rather than append beside it.
    assert!(
        !argv_line.contains("--system-prompt"),
        "a response profile must never replace the coding system prompt outright: {argv_line}"
    );

    // Clause 1: "a response profile can control user-facing communication" —
    // the settings document at the `--settings` path names the style
    // `concise-technical` maps to (`closest_output_style`: Concise verbosity
    // + Silent narration -> "Concise").
    let settings_path = argv_line
        .split("--settings")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("--settings carried no path: {argv_line}"));
    let settings = std::fs::read_to_string(settings_path.trim_matches('"'))
        .unwrap_or_else(|err| panic!("could not read {settings_path}: {err}"));
    let settings_json: serde_json::Value =
        serde_json::from_str(&settings).expect("the settings document is valid JSON");
    assert_eq!(
        settings_json["outputStyle"], "Concise",
        "the composed settings document must carry the resolved profile's native style: \
         {settings}"
    );

    // Clause 2: "without reducing verification" — exactly one
    // `--append-system-prompt` in argv, carrying the floor sentence
    // verbatim.
    assert_eq!(
        argv_line.matches("--append-system-prompt").count(),
        1,
        "the floor sentence must ride along through exactly one --append-system-prompt: \
         {argv_line}"
    );
    assert!(
        argv_line.contains(&floor_directive()),
        "the floor sentence — the reports a response profile may never reduce — must reach \
         argv verbatim: {argv_line}"
    );
}

// ---------------------------------------------------------------------------
// 1905 — two concurrent gateway instances, isolated
// ---------------------------------------------------------------------------

/// **1905** — "Consider gateway mode usable when two concurrent Glasshouse
/// instances can run isolated local gateways without port or credential
/// collisions."
///
/// Shape: `tests/gateway_degrade.rs`'s seam-level entry —
/// `glasshouse::gateway::start_if_required`, the exact function
/// `main.rs::launch_session` and `overlay_resolution` both call — run twice,
/// each with its own upstream and its own minted `GatewayToken`. Entry:
/// `phase-9g.md` ("the local gateway process": `Gateway::start` binds
/// `(loopback, 0)`, letting the OS choose the port, and mints one
/// `GatewayToken` per instance).
///
/// Mutation: `gateway/mod.rs::EPHEMERAL_PORT`, changed from `0` (ask the OS
/// for a free port) to a fixed port — "pin a fixed port," which makes the
/// second gateway's bind collide with the first's.
#[test]
fn v1_1905_two_concurrent_gateways_get_different_ports_and_reject_each_others_credential() {
    let upstream_a = FixtureUpstream::start();
    let upstream_b = FixtureUpstream::start();

    let backend = |name: &str, upstream: &FixtureUpstream| {
        let credential = test_credential(
            &format!("V1905_{}_KEY", name.to_ascii_uppercase()),
            "sk-planted-not-a-real-key",
        );
        UpstreamBackend::new(
            name.to_owned(),
            vec![Route::new(
                "anthropic-messages".to_owned(),
                &["/messages"],
                &upstream.base_url(),
            )],
            credential,
            CredentialId::new(
                name,
                SecretRef::Environment {
                    var: format!("V1905_{}_KEY", name.to_ascii_uppercase()),
                },
            ),
            Cost::Metered,
        )
        .expect("a loopback http URL is absolute and this credential is header-safe")
    };

    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;

    let gateway_a = glasshouse::gateway::start_if_required(&[profile.clone()], || {
        Ok(Upstream::with_failover(vec![backend("a", &upstream_a)]).unwrap())
    })
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");
    let gateway_b = glasshouse::gateway::start_if_required(&[profile], || {
        Ok(Upstream::with_failover(vec![backend("b", &upstream_b)]).unwrap())
    })
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");

    assert_ne!(
        gateway_a.address().port(),
        gateway_b.address().port(),
        "two concurrent gateways must not collide on the same port"
    );

    // Each instance serves a request under its own token.
    let response_a = send_and_read(
        gateway_a.address(),
        &messages_request(gateway_a.token().expose()),
    );
    assert!(
        response_a.starts_with("HTTP/1.1 200"),
        "gateway A must serve a request under its own token: {response_a}"
    );
    let response_b = send_and_read(
        gateway_b.address(),
        &messages_request(gateway_b.token().expose()),
    );
    assert!(
        response_b.starts_with("HTTP/1.1 200"),
        "gateway B must serve a request under its own token: {response_b}"
    );

    // A's token is rejected by B, and vice versa.
    let cross_a_on_b = send_and_read(
        gateway_b.address(),
        &messages_request(gateway_a.token().expose()),
    );
    assert!(
        cross_a_on_b.starts_with("HTTP/1.1 401"),
        "gateway B must reject gateway A's credential: {cross_a_on_b}"
    );
    let cross_b_on_a = send_and_read(
        gateway_a.address(),
        &messages_request(gateway_b.token().expose()),
    );
    assert!(
        cross_b_on_a.starts_with("HTTP/1.1 401"),
        "gateway A must reject gateway B's credential: {cross_b_on_a}"
    );

    assert_eq!(
        upstream_a.requests().len(),
        1,
        "{:?}",
        upstream_a.requests()
    );
    assert_eq!(
        upstream_b.requests().len(),
        1,
        "{:?}",
        upstream_b.requests()
    );
}

// ---------------------------------------------------------------------------
// 1906 — OpenRouter, one generic openai-compatible and one generic
// anthropic-compatible endpoint, all configured and tested
// ---------------------------------------------------------------------------

/// **1906** — "Consider provider setup usable when OpenRouter, one generic
/// OpenAI-compatible endpoint, and one generic Anthropic-compatible endpoint
/// can be configured and tested."
///
/// Shape: `glasshouse resources --probe <name>` — the connectivity test
/// `phase-9d.md` records COMPLETE (`main.rs::resources_report` →
/// `provider::resources::probe_provider` →
/// `provider::discovery::connectivity_with_headers`) — run through the
/// shipped binary against three real loopback fixtures, one per template.
/// Entry: `phase-9d.md` (the connectivity test) and `phase-9d-9a.md`
/// (OpenRouter's and the two generic templates' endpoints).
///
/// Mutation: `provider/mod.rs::templates`'s `"openai-compatible"` template
/// name literal, renamed — "break the generic template resolution," which
/// makes `[providers.*]\ntemplate = "openai-compatible"` resolve to no known
/// template and the probe report `not probed`.
#[test]
fn v1_1906_openrouter_and_both_generic_templates_are_configured_and_actually_probed() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    let openrouter_fixture = FixtureUpstream::start();
    let openai_fixture = FixtureUpstream::start();
    let anthropic_fixture = FixtureUpstream::start();

    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [providers.v1906-openrouter]\ntemplate = \"openrouter\"\n\
             base_url = \"{}/v1\"\ncredential_env = [\"V1906_OPENROUTER_KEY\"]\n\n\
             [providers.v1906-openai]\ntemplate = \"openai-compatible\"\n\
             base_url = \"{}\"\ncredential_env = [\"V1906_OPENAI_KEY\"]\n\n\
             [providers.v1906-anthropic]\ntemplate = \"anthropic-compatible\"\n\
             base_url = \"{}\"\ncredential_env = [\"V1906_ANTHROPIC_KEY\"]\n",
            openrouter_fixture.base_url(),
            openai_fixture.base_url(),
            anthropic_fixture.base_url(),
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&project)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .args([
            "resources",
            "--no-harness",
            "--probe",
            "v1906-openrouter",
            "--probe",
            "v1906-openai",
            "--probe",
            "v1906-anthropic",
        ])
        .env("V1906_OPENROUTER_KEY", "sk-planted-openrouter-1906")
        .env("V1906_OPENAI_KEY", "sk-planted-openai-1906")
        .env("V1906_ANTHROPIC_KEY", "sk-planted-anthropic-1906")
        .output()
        .expect("run glasshouse resources --probe");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);
    for name in ["v1906-openrouter", "v1906-openai", "v1906-anthropic"] {
        let row = report
            .lines()
            .find(|line| line.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("no probe row for {name}:\n{report}"));
        assert!(
            row.contains("reached (status 200)"),
            "{name} must have been actually reached, not merely configured: {row}"
        );
    }

    let openrouter_requests = openrouter_fixture.requests();
    assert_eq!(openrouter_requests.len(), 1, "{:?}", openrouter_requests);
    assert_eq!(openrouter_requests[0].target, "/v1/models");
    assert_eq!(
        openrouter_requests[0].header("authorization"),
        Some("Bearer sk-planted-openrouter-1906"),
        "the configured credential must be attached to the request that actually hit the base \
         URL: {:?}",
        openrouter_requests[0]
    );

    let openai_requests = openai_fixture.requests();
    assert_eq!(openai_requests.len(), 1, "{:?}", openai_requests);
    assert_eq!(
        openai_requests[0].header("authorization"),
        Some("Bearer sk-planted-openai-1906"),
        "{:?}",
        openai_requests[0]
    );

    let anthropic_requests = anthropic_fixture.requests();
    assert_eq!(anthropic_requests.len(), 1, "{:?}", anthropic_requests);
    assert_eq!(
        anthropic_requests[0].header("x-api-key"),
        Some("sk-planted-anthropic-1906"),
        "the anthropic-compatible template must attach the credential as x-api-key, not a \
         bearer token: {:?}",
        anthropic_requests[0]
    );
}

// ---------------------------------------------------------------------------
// 1907 — a free-tier model performs a disposable support job
// ---------------------------------------------------------------------------

/// **1907** — "Consider free-pool support usable when at least one
/// configured zero-cost or free-tier model can perform a disposable
/// Glasshouse support job."
///
/// Shape: `tests/classification_call.rs`'s pattern — a provider with one
/// model named under `free_models`, `routing.model = automatic`, and
/// `glasshouse classify <text>` run through the shipped binary against a
/// canned OpenAI chat-completions endpoint. Entry: `phase-9i.md`
/// ("free-pool routing, 9 of 14" — the disposable policy's caller).
///
/// Mutation: `main.rs::disposable_candidates`, discarding the provider's
/// declared `free_models` before any candidate is built — "refuse free
/// models for support jobs" — on the seam this packet may edit
/// (`routing/disposable.rs` is FORBIDDEN, live worker entitlement-pool's).
#[test]
fn v1_1907_a_free_tier_model_performs_the_classification_support_job() {
    let tmp = tempdir();
    let base = tmp.path();
    let project = git_project(base, "proj");
    let data_dir = base.join("data");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // Blocking accept on a dedicated thread, armed here before the subprocess
    // spawns below, matching `launch_preflight.rs`'s `FakeProvider` idiom: a
    // nonblocking poll can miss the connection window between polls, but a
    // blocking `accept()` cannot miss a connection the kernel has already
    // queued, so the server side of this race is gone by construction.
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback must bind");
    let address = listener.local_addr().unwrap();
    let (served_tx, served_rx) = std::sync::mpsc::channel();
    {
        let requests = Arc::clone(&requests);
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_ok() {
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                            break;
                        }
                        if let Some(v) = line
                            .to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(str::trim)
                            .and_then(|v| v.parse().ok())
                        {
                            length = v;
                        }
                    }
                    let mut body = vec![0u8; length];
                    let _ = reader.read_exact(&mut body);
                    requests
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&body).into_owned());
                    let content = "{\"needs_repo_context\":false,\"needs_code_modification\":false,\
                                    \"needs_shell_execution\":false,\"needs_browser_interaction\":false,\
                                    \"complexity\":\"trivial\",\"likely_multi_turn\":false,\
                                    \"workload_tier\":\"leaf\",\"safe_for_disposable_model\":true,\
                                    \"warm_context\":\"prefer_warm\",\"confidence\":\"high\"}";
                    let document = serde_json::json!({
                        "choices": [{ "message": { "role": "assistant", "content": content } }],
                        "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{document}",
                        document.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
            }
            // The subprocess connects only once its request has been fully
            // read and answered above, so a receiver that never fires means
            // the connection this test guards never happened at all.
            let _ = served_tx.send(());
        });
    }

    let runtime = bootstrap_at(&data_dir, &config_dir, &project);
    let mut user = UserConfig::load(runtime.paths()).unwrap();
    let mut provider = ProviderConfig::new("openai-compatible");
    provider.set_base_url(Some(format!("http://{address}/v1")));
    provider.set_credential_env(vec!["V1907_KEY".to_owned()]);
    provider.set_free_models(vec!["v1907-free-model".to_owned()]);
    user.providers_mut().set("v1907-provider", provider);
    user.routing_mut()
        .set_model(Some(RoutingModelChoice::Automatic));
    user.save(runtime.paths()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .env("V1907_KEY", "sk-planted-not-a-real-key-1907")
        .arg("--scope")
        .arg(&project)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .arg("classify")
        .arg("what changed in this diff?")
        .output()
        .expect("run glasshouse classify");
    // The subprocess has already exited, so the accept thread is either done
    // or never going to hear from anyone; a generous bound just keeps a
    // genuinely-missed connection from hanging the test instead of failing
    // the assertions below.
    let _ = served_rx.recv_timeout(Duration::from_secs(5));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        report.contains("v1907-free-model") || report.to_ascii_lowercase().contains("model"),
        "the classification report must say a model answered: {report}"
    );

    let seen = requests.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "the free-tier model's own endpoint must have been asked exactly once: {seen:?}"
    );
    assert!(
        seen[0].contains("v1907-free-model"),
        "the request must have named the free model this provider declared: {}",
        seen[0]
    );
}

// ---------------------------------------------------------------------------
// 1908 — cross-platform CI legs
// ---------------------------------------------------------------------------

/// **1908** — "Consider cross-platform support stable only after PTY/session
/// smoke tests pass on macOS, Linux, and native Windows CI runners."
///
/// This machine is macOS. The local smoke this packet can actually run is
/// this file's own suite (all ten tests above) plus
/// `cargo test -p glasshouse --all-features --test pty_smoke`, quoted with
/// its `test result:` line in this packet's facts block — not reproduced as
/// a `#[test]` here, since a test in this file cannot invoke `cargo test` on
/// a sibling target.
///
/// **`worker-capabilities.md`: "Hide missing platform evidence behind
/// injected-platform unit tests" is exactly what this criterion forbids
/// faking**, and no CI-run evidence for Linux or native Windows exists in
/// any evidence-ledger entry this packet's `READ ONLY THIS` names
/// (`docs/product/evidence/README.md`'s index; no phase file in it records a
/// Linux or Windows CI run of the pty smoke suite). This line is reported
/// `open`, naming exactly the missing legs: a Linux CI run and a native
/// Windows CI run of `tests/pty_smoke.rs`, neither of which this worktree
/// can produce.
#[test]
fn v1_1908_cross_platform_ci_legs_stay_open_pending_linux_and_windows_runs() {
    // No assertion to make from macOS alone — this test exists so the
    // criterion has a line in this suite's output, and the honest verdict is
    // recorded in the packet's facts block rather than invented here.
}
