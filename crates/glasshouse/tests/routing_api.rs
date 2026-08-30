//! Phase 42, the door's two routing verbs: capability map line 1680,
//! "allow the API to retrieve the current routing-model selection and
//! health", and line 1681, "allow the API to request an inspectable routing
//! recommendation without executing it."
//!
//! `mod api` is declared from `main.rs`, so — exactly as
//! `session_model.rs`'s own `control_api` module and `capacity_api.rs`
//! explain — nothing outside the binary can reach the control door any other
//! way. This drives `glasshouse api serve` for real over its Unix domain
//! socket, following `capacity_api.rs`'s own fixture shape: the 1680 tests
//! need no harness at all, and the 1681 tests configure one only so that
//! there is a candidate set to rank — and so that "it was never started" is
//! something a test can observe rather than assume.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        Self { _tmp: tmp, base }
    }

    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }

    /// Write a project-level config file directly — this test proves the API
    /// reads it, not the settings UI that writes it, which belongs to a
    /// different package. Mirrors `capacity_api.rs`'s own
    /// `write_project_config`.
    fn write_project_config(&self, root: &Path, toml: &str) {
        let dir = root.join(".glasshouse");
        std::fs::create_dir_all(&dir).expect("create .glasshouse dir");
        std::fs::write(dir.join("config.toml"), toml).expect("write project config");
    }
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    fn start(fixture: &Fixture, root: &Path) -> Self {
        Self::start_with_env(fixture, root, &[])
    }

    fn start_with_env(fixture: &Fixture, root: &Path, env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(fixture.base.join("data"))
            .arg("--config-dir")
            .arg(fixture.base.join("config"))
            .arg("api")
            .arg("serve")
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn `glasshouse api serve`");

        let stderr = child.stderr.take().expect("captured stderr");
        let mut reader = BufReader::new(stderr);
        let deadline = Instant::now() + TIMEOUT;
        let socket = loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).expect("read server stderr");
            assert!(read > 0, "the server exited before announcing its socket");
            if let Some(path) = line
                .trim_end()
                .strip_prefix("glasshouse: control API listening on ")
            {
                break PathBuf::from(path);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the server to announce its socket"
            );
        };

        Self { child, socket }
    }

    fn call(&self, request: serde_json::Value) -> serde_json::Value {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => break stream,
                Err(err) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out connecting to the control socket: {err}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut payload = serde_json::to_string(&request).expect("encode request");
        payload.push('\n');
        stream.write_all(payload.as_bytes()).expect("write request");

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim_end()).expect("parse response")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A project with no recorded routing preference gets the honest default —
/// deterministic heuristics, `not_configured` — not an error and not a
/// fabricated pin. Box 1680, requirement 3.
#[test]
fn the_default_project_reports_its_default_selection_and_layer() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    assert_eq!(result["selection"]["choice"], "deterministic", "{result}");
    assert!(result["selection"]["provider"].is_null(), "{result}");
    assert!(result["selection"]["model"].is_null(), "{result}");
    assert_eq!(result["layer"], "default", "{result}");
    assert_eq!(result["resolution"]["state"], "heuristics", "{result}");
    assert_eq!(result["resolution"]["reason"], "not_configured", "{result}");
}

/// A pinned routing model, naming a provider that is actually configured,
/// round-trips through the door with its provider and model intact, and
/// resolves rather than degrading. Box 1680, requirement 1 and 2.
#[test]
fn a_pinned_routing_model_round_trips_through_the_door() {
    let fixture = Fixture::new();
    let root = fixture.project_root("beta");
    fixture.write_project_config(
        &root,
        "version = 1\n\n\
         [providers.anyrouter]\n\
         template = \"anyrouter\"\n\n\
         [routing.model]\n\
         kind = \"pinned\"\n\
         provider = \"anyrouter\"\n\
         model = \"claude-opus\"\n",
    );
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    assert_eq!(result["selection"]["choice"], "pinned", "{result}");
    assert_eq!(result["selection"]["provider"], "anyrouter", "{result}");
    assert_eq!(result["selection"]["model"], "claude-opus", "{result}");
    assert_eq!(result["layer"], "project", "{result}");
    assert_eq!(result["resolution"]["state"], "pinned", "{result}");
    assert_eq!(result["resolution"]["provider"], "anyrouter", "{result}");
    assert_eq!(result["resolution"]["model"], "claude-opus", "{result}");
}

/// A pin naming a provider that is not configured degrades to heuristics
/// with the reason named in `RoutingFallback`'s own words — not an error and
/// not a silent success that pretends the pin still applies. Box 1680,
/// requirement 2.
#[test]
fn a_pin_naming_an_unconfigured_provider_degrades_to_heuristics_with_the_reason() {
    let fixture = Fixture::new();
    let root = fixture.project_root("gamma");
    fixture.write_project_config(
        &root,
        "version = 1\n\n\
         [routing.model]\n\
         kind = \"pinned\"\n\
         provider = \"ghost-provider\"\n\
         model = \"ghost-model\"\n",
    );
    let server = Server::start(&fixture, &root);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    // The recorded choice is still reported honestly...
    assert_eq!(result["selection"]["choice"], "pinned", "{result}");
    assert_eq!(
        result["selection"]["provider"], "ghost-provider",
        "{result}"
    );
    assert_eq!(result["selection"]["model"], "ghost-model", "{result}");
    assert_eq!(result["layer"], "project", "{result}");
    // ...but the resolution says plainly that it cannot be honored.
    assert_eq!(result["resolution"]["state"], "heuristics", "{result}");
    assert_eq!(
        result["resolution"]["reason"], "provider_not_configured",
        "{result}"
    );
    assert_eq!(
        result["resolution"]["provider"], "ghost-provider",
        "{result}"
    );
    assert_eq!(result["resolution"]["model"], "ghost-model", "{result}");
}

/// A provider's credential lives behind an environment variable named in
/// `credential_env` — never a value this door reads or could echo back.
/// `RoutingModelChoice::Pinned` only ever carries a provider name and a
/// model name (see its own doc comment), so this asserts the negative
/// directly against the raw wire response rather than trusting the type by
/// inspection alone. Security invariant from the packet.
#[test]
fn no_credential_value_appears_in_the_routing_model_response() {
    let fixture = Fixture::new();
    let root = fixture.project_root("delta");
    fixture.write_project_config(
        &root,
        "version = 1\n\n\
         [providers.anyrouter]\n\
         template = \"anyrouter\"\n\
         credential_env = [\"ROUTING_API_TEST_SECRET\"]\n\n\
         [routing.model]\n\
         kind = \"pinned\"\n\
         provider = \"anyrouter\"\n\
         model = \"claude-opus\"\n",
    );
    const SECRET: &str = "sk-do-not-leak-BB6B6E9F3C9E4E39A9E9";
    let server = Server::start_with_env(&fixture, &root, &[("ROUTING_API_TEST_SECRET", SECRET)]);

    let response = server.call(serde_json::json!({ "op": "routing_model" }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let raw = serde_json::to_string(&response).expect("serialize response");
    assert!(
        !raw.contains(SECRET),
        "the routing-model response must never carry a credential value: {raw}"
    );
}

// --- GH-ROUTE-RECOMMEND: line 1681, the recommendation that executes nothing --
//
// `glasshouse route --task "..."` already answers "where would this work go,
// and why" for a person. Line 1681 asks the same question through the socket,
// and adds a contract the command does not have to state because a command
// that printed a report obviously started nothing: **the door must not
// execute it**. A verb that recommends perfectly and quietly records
// something has failed this line, so the proof below is a negative one —
// three stores and the harness itself, unchanged across a call.

/// The fixture's provider credential variable, copied from
/// `tests/route_command.rs`. A name only; nothing here resolves a value.
const ROUTE_CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTE_RECOMMEND_TEST_KEY";

/// The value planted in [`ROUTE_CREDENTIAL_VAR`], so the security assertion
/// has something specific to look for rather than a shape.
const ROUTE_CREDENTIAL_VALUE: &str = "sk-do-not-leak-route-4F1D9C2A7E634B08";

/// `unix::MAX_ROUTE_ALTERNATIVES`, restated here because `api` is declared
/// from `main.rs` and no test can import a constant out of the binary. A
/// change to one without the other fails
/// `an_absurd_alternatives_bound_still_comes_back_bounded` loudly, which is
/// the point of asserting the exact number rather than "not too many".
const MAX_ROUTE_ALTERNATIVES: usize = 20;

impl Fixture {
    /// A user-level config, which is where `tests/route_command.rs` puts the
    /// harnesses and profiles its own routing fixture needs. Same layer, same
    /// file name, so a report from `glasshouse route` and one from this door
    /// are answered off identical configuration.
    fn write_user_config(&self, toml: &str) {
        std::fs::write(self.base.join("config").join("config.toml"), toml)
            .expect("write user config");
    }

    /// A harness executable that records every argv it is started with and
    /// exits. It is never expected to run in this file — that is the
    /// assertion — and recording argv unconditionally is what makes "it never
    /// ran" observable rather than assumed.
    fn install_fake_harness(&self, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin = self.base.join("bin");
        std::fs::create_dir_all(&bin).expect("create bin dir");
        let path = bin.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                self.harness_argv_log().display()
            ),
        )
        .expect("write fake harness");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake harness")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake harness");
        path
    }

    fn harness_argv_log(&self) -> PathBuf {
        self.base.join("harness-argv.log")
    }

    /// Every argv a configured harness has been started with. Empty when the
    /// log does not exist, which is the state this file requires.
    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(self.harness_argv_log()) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Run the shipped binary as a person would, against the same project,
    /// data directory and config directory the server was started for — the
    /// half of the CLI/door agreement test that is not the door.
    fn glasshouse(&self, root: &Path, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(ROUTE_CREDENTIAL_VAR, ROUTE_CREDENTIAL_VALUE)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Every `(provider, model, route, context_state)` identity with rows in
    /// `routing_observations` — the store a recommendation must not write to,
    /// read the way the shipped binary reads it rather than by inspecting
    /// the file.
    ///
    /// The window is every timestamp there is, because
    /// `observed_identities` filters on `observed_at` between
    /// `now - window` and `now`: anything narrower turns "the verb recorded
    /// something" into "the verb recorded something outside the window",
    /// which is a passing test for a broken contract. The first draft of
    /// this helper got exactly that wrong — `now = i64::MAX / 4` with
    /// `window = i64::MAX / 8` puts the *earliest* bound at `i64::MAX / 8`,
    /// above every real timestamp — and the required mutation caught it by
    /// surviving.
    fn routing_observations(&self, root: &Path) -> Vec<String> {
        use clap::Parser as _;
        use glasshouse::routing::evidence::EvidenceLedger;

        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().expect("utf-8 data dir"),
            "--config-dir",
            self.base.join("config").to_str().expect("utf-8 config dir"),
        ])
        .expect("parse a bare CLI for the test's own runtime");
        let runtime = glasshouse::bootstrap(&cli, root).expect("bootstrap the project");
        let ledger = EvidenceLedger::open(&runtime).expect("open the evidence ledger");
        ledger
            .observed_identities(i64::MAX / 4, i64::MAX / 4, 1000)
            .expect("read observed routing identities")
            .iter()
            .map(|identity| format!("{identity:?}"))
            .collect()
    }
}

/// Two harnesses behind one provider, differing only in what map line 1382's
/// registry establishes about `browser-use` — `tests/route_command.rs`'s own
/// `TwoHarnessFixture` configuration, so the flip this file asserts through
/// the socket is the same flip that file asserts through stdout.
fn two_harness_config(claude_code: &Path, codex: &Path) -> String {
    let escape = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    format!(
        "version = 1\n\n\
         [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
         [integrations.codex]\nenabled = true\nexecutable = \"{}\"\n\n\
         [providers.route-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{ROUTE_CREDENTIAL_VAR}\"]\n\n\
         [profiles.direct-cc]\nharness = \"claude-code\"\n\n\
         [profiles.direct-cc.backend]\nkind = \"direct-provider\"\n\
         provider = \"route-probe\"\n\n\
         [profiles.direct-codex]\nharness = \"codex\"\n\
         expected_protocol = \"openai-chat\"\n\n\
         [profiles.direct-codex.backend]\nkind = \"direct-provider\"\n\
         provider = \"route-probe\"\n",
        escape(claude_code),
        escape(codex),
    )
}

/// A project set up the way `two_harness_config` describes, with the server
/// already listening.
fn two_harness_project(fixture: &Fixture, name: &str) -> (PathBuf, Server) {
    let root = fixture.project_root(name);
    let claude_code = fixture.install_fake_harness("fake-claude-code");
    let codex = fixture.install_fake_harness("fake-codex");
    fixture.write_user_config(&two_harness_config(&claude_code, &codex));
    let server = Server::start_with_env(
        fixture,
        &root,
        &[
            (ROUTE_CREDENTIAL_VAR, ROUTE_CREDENTIAL_VALUE),
            ("PATH", fixture.base.join("empty-path").to_str().unwrap()),
        ],
    );
    (root, server)
}

/// Acceptance test 1, and ruling 3. The door answers with a destination
/// **and** the contributions behind it: a bare identifier is not
/// "inspectable", which is the word line 1681 uses.
#[test]
fn a_recommendation_names_a_destination_and_the_contributions_behind_it() {
    let fixture = Fixture::new();
    let (_root, server) = two_harness_project(&fixture, "recommend");

    let response = server.call(serde_json::json!({
        "op": "recommend_route",
        "task": "explain what chrome browser screenshot support looks like",
    }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let result = &response["result"];
    assert_eq!(result["routed"], true, "{result}");
    assert_eq!(result["moment"], "session-start", "{result}");
    assert_eq!(
        result["destination"]["id"], "fresh:claude-code:direct-cc",
        "{result}"
    );
    assert_eq!(result["destination"]["harness"], "claude-code", "{result}");
    assert_eq!(
        result["destination"]["launch_profile"], "direct-cc",
        "{result}"
    );
    assert_eq!(result["destination"]["fresh"], true, "{result}");

    let contributions = result["contributions"]
        .as_array()
        .unwrap_or_else(|| panic!("contributions must be an array: {result}"));
    let names: Vec<&str> = contributions
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(
        names.contains(&"harness capability fit")
            && names.contains(&"capability fit")
            && names.contains(&"session affinity"),
        "every scoring term the router weighed must appear by name: {names:?}"
    );

    // Ruling 3's actual content: the *evidence*, not only the term's name and
    // number. This is the capability contribution this batch added, and it
    // must say what the task asked for.
    let capability = contributions
        .iter()
        .find(|c| c["name"] == "capability fit")
        .unwrap_or_else(|| panic!("no capability term: {result}"));
    let evidence = capability["evidence"]
        .as_str()
        .unwrap_or_else(|| panic!("capability evidence must be a string: {result}"));
    assert!(
        evidence.contains("needs browser interaction"),
        "the capability contribution must name the hard capability the task implied, not \
         only score it: {evidence}"
    );

    // And the alternatives carry their own explanations, which is what makes
    // "why this one and not that one" answerable. Four candidates, not two:
    // each harness contributes its implied `native` profile alongside the two
    // the config names, exactly as `glasshouse route` ranks them.
    let alternatives = result["alternatives"]
        .as_array()
        .unwrap_or_else(|| panic!("alternatives must be an array: {result}"));
    let runner_up = alternatives
        .iter()
        .find(|entry| entry["destination"]["id"] == "fresh:codex:direct-codex")
        .unwrap_or_else(|| panic!("the other configured profile must be ranked: {result}"));
    assert!(
        !runner_up["contributions"]
            .as_array()
            .expect("alternative contributions must be an array")
            .is_empty(),
        "an alternative without its own contributions is a ranking a caller cannot check: \
         {result}"
    );
    assert_eq!(
        result["alternatives_omitted"], 0,
        "four candidates fit under the default bound of five, so nothing may be reported as \
         omitted: {result}"
    );

    // And what the ranking could *not* see, which is the other half of being
    // inspectable: a `0.000` a caller cannot tell from an unread input is a
    // number it will misread.
    let caveats = result["caveats"]
        .as_str()
        .unwrap_or_else(|| panic!("caveats must be a string: {result}"));
    assert!(
        caveats.contains("provider health")
            && caveats.contains("no gateway has yet persisted a health reading"),
        "the response must say that the provider-health term was never read rather than \
         weighed and found equal. **This caveat became conditional with line 1599's \
         bridge** — `main.rs::observed_provider_health` now fills the pool from \
         `GatewayHealthCache`, so the line is printed only when nothing was attributed, \
         and this project has persisted nothing: {caveats}"
    );
}

/// Acceptance test 2, and the whole of line 1681's *"without executing it"*.
///
/// Four observations, taken before and after one call: the session list, the
/// event log, `routing_observations`, and whether the configured harness was
/// ever started. Each is a different way for a recommendation to stop being a
/// recommendation, and this line's contract is that none of them moves.
#[test]
fn a_recommendation_executes_nothing_and_records_nothing() {
    let fixture = Fixture::new();
    let (root, server) = two_harness_project(&fixture, "inert");

    let sessions_before = server.call(serde_json::json!({ "op": "list_sessions" }));
    let events_before =
        server.call(serde_json::json!({ "op": "events", "after": 0, "limit": 1000 }));
    let observations_before = fixture.routing_observations(&root);
    assert_eq!(sessions_before["status"], "ok", "{sessions_before}");
    assert_eq!(events_before["status"], "ok", "{events_before}");

    let response = server.call(serde_json::json!({
        "op": "recommend_route",
        "task": "review the repository and run the shell command that builds it",
    }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");
    assert_eq!(response["result"]["routed"], true, "{response}");

    let sessions_after = server.call(serde_json::json!({ "op": "list_sessions" }));
    let events_after =
        server.call(serde_json::json!({ "op": "events", "after": 0, "limit": 1000 }));
    let observations_after = fixture.routing_observations(&root);

    assert_eq!(
        sessions_before, sessions_after,
        "a routing recommendation must not create, close or touch a session"
    );
    assert_eq!(
        events_before, events_after,
        "a routing recommendation must not put anything in this project's event log"
    );
    assert_eq!(
        observations_before, observations_after,
        "a routing recommendation must not write a routing observation: a verb that decides \
         nothing has nothing to observe"
    );
    assert!(
        fixture.harness_invocations().is_empty(),
        "the configured harness must never be started by a recommendation: {:?}",
        fixture.harness_invocations()
    );
}

/// Acceptance test 3, through the socket: two calls differing **only** in
/// task text recommend differently, because the capability registry the task
/// reaches actually separates these two candidates. This is
/// `route_command.rs`'s `a_task_naming_a_capability_flips_which_candidate_the_
/// ranking_prefers`, asked over the door.
#[test]
fn two_tasks_differing_only_in_text_are_recommended_different_destinations() {
    let fixture = Fixture::new();
    let (_root, server) = two_harness_project(&fixture, "flip");

    let plain = server.call(serde_json::json!({ "op": "recommend_route" }));
    assert_eq!(plain["status"], "ok", "unexpected response: {plain}");
    assert_eq!(
        plain["result"]["destination"]["id"], "fresh:codex:direct-codex",
        "without a task, `direct-codex`'s protocol-fit edge must be what wins — the \
         fixture's own baseline: {plain}"
    );

    let browser = server.call(serde_json::json!({
        "op": "recommend_route",
        "task": "explain what chrome browser screenshot support looks like",
    }));
    assert_eq!(browser["status"], "ok", "unexpected response: {browser}");
    assert_eq!(
        browser["result"]["destination"]["id"], "fresh:claude-code:direct-cc",
        "a task naming only browser interaction must close that lead and change which \
         candidate wins: {browser}"
    );
}

/// Acceptance test 4, and ruling 2 made executable. `glasshouse route --task
/// X` and this verb with the same task must name the same destination —
/// which they cannot fail to do while there is one ranking, and could
/// quietly fail to do the moment there are two.
///
/// Compared on the door's own `report`, which is `Routed::render`'s first
/// line, against the command's first line of stdout: identical text from
/// identical inputs, not two parses of the same idea.
#[test]
fn the_command_and_the_door_recommend_the_same_destination() {
    let fixture = Fixture::new();
    let (root, server) = two_harness_project(&fixture, "agree");

    for task in [
        "explain what chrome browser screenshot support looks like",
        "what is the difference between a fresh and an existing session",
    ] {
        let response = server.call(serde_json::json!({
            "op": "recommend_route",
            "task": task,
        }));
        assert_eq!(response["status"], "ok", "unexpected response: {response}");

        let from_door = response["result"]["report"]
            .as_str()
            .unwrap_or_else(|| panic!("report must be a string: {response}"))
            .lines()
            .next()
            .expect("the report must have a first line")
            .to_owned();
        let from_command = fixture.glasshouse(&root, &["route", "--task", task]);
        let first = from_command
            .lines()
            .next()
            .expect("`glasshouse route` must print a first line");

        assert_eq!(
            from_door, first,
            "`glasshouse route --task` and the door must name the same destination for the \
             same task; the command printed:\n{from_command}"
        );
        assert_eq!(
            response["result"]["destination"]["id"]
                .as_str()
                .map(|id| first.contains(id)),
            Some(true),
            "the structured destination and the rendered one must be the same destination: \
             {response}"
        );
    }
}

/// Acceptance test 5, and ruling 4. A caller may lower the door's ceiling and
/// cannot raise it, so an absurd bound comes back at the ceiling with the
/// remainder counted rather than silently dropped.
///
/// The fixture configures more launch profiles than the ceiling on purpose:
/// against a two-candidate project this assertion would pass on a build with
/// no ceiling at all.
#[test]
fn an_absurd_alternatives_bound_still_comes_back_bounded() {
    let fixture = Fixture::new();
    let root = fixture.project_root("bounded");
    let claude_code = fixture.install_fake_harness("fake-claude-code");
    let escaped = claude_code.display().to_string().replace('\\', "\\\\");

    const PROFILES: usize = 25;
    let mut config = format!(
        "version = 1\n\n\
         [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
         [providers.route-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{ROUTE_CREDENTIAL_VAR}\"]\n"
    );
    for index in 0..PROFILES {
        config.push_str(&format!(
            "\n[profiles.p{index:02}]\nharness = \"claude-code\"\n\n\
             [profiles.p{index:02}.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n"
        ));
    }
    fixture.write_user_config(&config);
    let server = Server::start_with_env(
        &fixture,
        &root,
        &[(ROUTE_CREDENTIAL_VAR, ROUTE_CREDENTIAL_VALUE)],
    );

    let response = server.call(serde_json::json!({
        "op": "recommend_route",
        "alternatives": u64::MAX,
    }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let returned = response["result"]["alternatives"]
        .as_array()
        .unwrap_or_else(|| panic!("alternatives must be an array: {response}"))
        .len();
    let omitted = response["result"]["alternatives_omitted"]
        .as_u64()
        .unwrap_or_else(|| panic!("alternatives_omitted must be a number: {response}"))
        as usize;
    assert_eq!(
        returned, MAX_ROUTE_ALTERNATIVES,
        "an unbounded ask must be answered at the door's ceiling: {response}"
    );
    // Derived, not assumed: the candidate set is the configured profiles plus
    // the harness's own implied `native` one, and this test is about the
    // ceiling rather than about that arithmetic.
    let candidates = returned + omitted + 1;
    assert!(
        candidates > PROFILES,
        "the fixture must offer more candidates than the ceiling, or this assertion would \
         pass against a door with no ceiling at all: {response}"
    );
    assert!(
        omitted > 0,
        "what the ceiling left out must be counted, not dropped in silence: {response}"
    );

    // And the default, which is the number a caller gets for asking nothing.
    let default = server.call(serde_json::json!({ "op": "recommend_route" }));
    assert_eq!(default["status"], "ok", "unexpected response: {default}");
    assert_eq!(
        default["result"]["alternatives"]
            .as_array()
            .expect("alternatives must be an array")
            .len(),
        5,
        "the default bound is five: {default}"
    );
    assert_eq!(
        default["result"]["alternatives_omitted"],
        serde_json::json!(candidates - 1 - 5),
        "the same candidate set, counted against the default bound: {default}"
    );
}

/// A moment this door does not know is refused, and the refusal does not
/// quote the caller's own string back at it. The security invariant is small
/// but it is the one that generalises: nothing a caller sends arrives in a
/// response verbatim.
#[test]
fn an_unrecognised_moment_is_refused_without_echoing_it() {
    let fixture = Fixture::new();
    let (_root, server) = two_harness_project(&fixture, "moment");

    let response = server.call(serde_json::json!({
        "op": "recommend_route",
        "moment": "whenever-you-like-9F2C",
    }));
    assert_eq!(
        response["status"], "error",
        "unexpected response: {response}"
    );

    let message = response["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error must carry a message: {response}"));
    assert!(
        message.contains("session-start")
            && message.contains("task-boundary")
            && message.contains("mid-turn"),
        "the refusal must name the moments that do exist: {message}"
    );
    assert!(
        !message.contains("whenever-you-like-9F2C"),
        "the refusal must not echo the caller's own string back: {message}"
    );
}

/// The security invariant from the packet, asserted against the raw wire
/// bytes rather than by inspecting the types: no credential value, and no
/// filesystem path, in a routing recommendation.
///
/// The harness executable's absolute path is in this project's configuration
/// and is exactly the kind of value a `Debug`-formatted destination would
/// carry out — this asserts it does not, alongside the credential.
#[test]
fn no_credential_and_no_path_appears_in_a_routing_recommendation() {
    let fixture = Fixture::new();
    let root = fixture.project_root("secrets");
    let claude_code = fixture.install_fake_harness("fake-claude-code");
    let codex = fixture.install_fake_harness("fake-codex");
    fixture.write_user_config(&two_harness_config(&claude_code, &codex));
    let server = Server::start_with_env(
        &fixture,
        &root,
        &[(ROUTE_CREDENTIAL_VAR, ROUTE_CREDENTIAL_VALUE)],
    );

    let response = server.call(serde_json::json!({
        "op": "recommend_route",
        "task": "read the repository and open a browser",
    }));
    assert_eq!(response["status"], "ok", "unexpected response: {response}");

    let raw = serde_json::to_string(&response).expect("serialize response");
    assert!(
        !raw.contains(ROUTE_CREDENTIAL_VALUE),
        "a routing recommendation must never carry a credential value: {raw}"
    );
    // The variable's *name* is a different matter and is deliberately not
    // asserted against: `known quota pressure` and `provider health` identify
    // the resource they had nothing to say about by `CredentialId::label`,
    // which `Destination::label`'s own doc comment states is a name and never
    // a value. Asserting the name away here would be asserting against the
    // explanation `glasshouse route` already prints.
    assert!(
        !raw.contains(&claude_code.display().to_string()),
        "the configured harness's absolute path must not travel in a response: {raw}"
    );
    assert!(
        !raw.contains(&root.display().to_string()),
        "nor the project root's: {raw}"
    );
}
