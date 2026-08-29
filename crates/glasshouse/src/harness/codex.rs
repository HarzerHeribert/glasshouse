//! Codex.
//!
//! Read from Codex 0.149.0 as installed on the development machine on
//! 2026-08-25 — `codex --help`, `codex resume --help`, `codex login --help`,
//! the hook state it records in its own configuration, and the session
//! rollouts it writes.

use std::ffi::OsString;

use super::{
    ApprovalMode, ApprovalModes, BackendSelection, Backends, Capabilities, CredentialPlacement,
    Declared, DirectProviderPlan, DirectProviderRequest, HarnessAdapter, HarnessDescription,
    HookCommand, HookDestination, HookInstallation, Hooks, Invocation, ModelOverride,
    NativeSessionKind, NativeSessionRecord, NativeSessionSource, RecordPerSessionSource,
    SandboxSelector, SessionIds, Vendor, WireProtocol, pairing::OfficialModelSupport,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codex;

/// Codex's own hook event catalogue, in the spelling its `hooks.json` uses.
///
/// Read from Codex 0.149.1's **hook review screen**, which enumerates every
/// event it supports with a one-line description — the authoritative artifact,
/// and not the one an earlier revision of this file cited.
///
/// That earlier revision listed ten events in `snake_case`, taken from the
/// `[hooks.state."<path>:<event>:0:0"]` keys in `config.toml`. Those keys are
/// real, but they are the spelling Codex uses to *record trust*, not the
/// spelling it reads from a hooks document — a hooks file on this machine used
/// PascalCase. The wrong artifact was cited, the casing was wrong throughout,
/// and `SessionEnd` was missing altogether.
const HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];
/// The events Glasshouse asks Codex to report.
///
/// A subset of [`HOOK_EVENTS`], deliberately not the remaining per-tool
/// events (`PreToolUse`/`PostToolUse`/`SubagentStart`/`SubagentStop`): those
/// fire many times per turn and say nothing about a *session's* state, the
/// same reasoning Claude Code's adapter applies. `SessionEnd` is asked for
/// here even though `session/lifecycle.rs` deliberately never maps it to a
/// state — Codex still reports it, and declining to *ask* for it would be a
/// second, redundant way of encoding the same decision.
///
/// `PreCompact`/`PostCompact` are asked for here even though they, too, are
/// not a `SessionLifecycle` state — a session that compacts was running
/// before and is running after. They are requested anyway so a real Codex
/// session registers a command for them and the event is observed (logged
/// by `RawObservation::preserve()`), which is a distinct, narrower claim
/// than *recording* a compaction durably. See `docs/product/evidence/phase-8.md`.
const REPORTED_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "Stop",
    "SessionEnd",
    "PreCompact",
    "PostCompact",
];

/// Seconds a reporting hook may take before Codex abandons it.
///
/// Codex clamps a declared timeout to 3 seconds — a real installation
/// declaring 10 produced `⚠ clamping SessionEnd hook timeout to 3s in
/// <project>/.codex/hooks.json`. Declaring 3 here means a real installation
/// produces no warning the user has to wonder about.
const HOOK_TIMEOUT_SECONDS: u32 = 3;

const PROTOCOLS: &[WireProtocol] = &[WireProtocol::OpenAiResponses];

const MODEL_OVERRIDE: &[ModelOverride] = &[
    ModelOverride::CommandLine("--model"),
    ModelOverride::Configuration("-c model=<id>"),
];

/// The only `wire_api` Codex 0.149.1 still accepts.
///
/// Not a preference — the other value is gone. `wire_api = "chat"` produces
/// ``Error loading config.toml: `wire_api = "chat"` is no longer supported.``
/// A provider serving only `openai-chat` therefore cannot back Codex at all,
/// and [`Codex::direct_provider_launch`] answers `None` for it rather than
/// composing a configuration Codex would reject after the process had already
/// started.
const WIRE_API: &str = "responses";

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::CommandLineArguments(
        "-c <key>=<value> overrides any config value; --oss and --local-provider select a \
         local backend",
    ),
    BackendSelection::GeneratedConfiguration(
        "-p/--profile layers $CODEX_HOME/<name>.config.toml over the base user config",
    ),
    BackendSelection::ChildEnvironment("CODEX_HOME relocates the whole configuration root"),
];

/// Codex 0.149.1's complete `codex --help` was read on 2026-08-26. It
/// documented no output-style, persona, or tone mechanism. The capability map
/// names "Codex personalities" only as an example, so this is unknown rather
/// than an invented declaration of support.
const COMMUNICATION_STYLE: Declared<super::CommunicationStyle> = Declared::Unverified;

/// The `{ "N" = "V", ... }` inline TOML table for `headers`, or `None` when
/// there are none to send — composed by hand rather than through a
/// TOML-writing dependency, exactly as `-c model_providers.<id>.http_headers`
/// was probed on Codex 0.149.1: `-c 'model_providers.<id>.http_headers={
/// "Name" = "value" }'` arrived, and `--strict-config` accepted the key.
///
/// A header *name* is already restricted to `[A-Za-z0-9-]` by
/// `crate::config`'s validation before it ever reaches this adapter, so it
/// needs no escaping. A header *value* still can — `\` and `"` are the two
/// characters that would let it break out of its own TOML string — so both
/// are escaped here.
fn http_headers_table(headers: &[(String, String)]) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    let mut table = String::from("{ ");
    for (index, (name, value)) in headers.iter().enumerate() {
        if index > 0 {
            table.push_str(", ");
        }
        table.push('"');
        table.push_str(name);
        table.push_str("\" = \"");
        table.push_str(&value.replace('\\', "\\\\").replace('"', "\\\""));
        table.push('"');
    }
    table.push_str(" }");
    Some(table)
}

/// The model family OpenAI produces for Codex, as Codex's own availability
/// record spells it.
const NATIVE_FAMILIES: &[&str] = &["gpt-5"];

/// A model Codex's own help documents as a value for `model`, outside the
/// family it ships as its default line.
const SUPPORTED_MODELS: &[&str] = &["o3"];

impl HarnessAdapter for Codex {
    fn id(&self) -> IntegrationId {
        IntegrationId::Codex
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn start(&self) -> Invocation {
        // "If no subcommand is specified, options will be forwarded to the
        // interactive CLI." Bare `codex` is the interactive session.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `codex resume [OPTIONS] [SESSION_ID] [PROMPT]` — "Session id (UUID)
        // or session name. UUIDs take precedence if it parses."
        //
        // Note the shape difference from Claude Code: a subcommand, not a
        // flag. This is exactly the harness-specific knowledge that would
        // otherwise be a `match` somewhere in core.
        Some(Invocation::of(["resume", native_session]))
    }

    /// A custom provider composed entirely out of `-c` overrides, so **no
    /// file is written at all** — not `~/.codex/config.toml`, not a generated
    /// profile beside it, not anything.
    ///
    /// That is the strongest possible form of "avoid overwriting the user's
    /// normal configuration": there is nothing to overwrite and nothing to
    /// clean up. Every override below was accepted by Codex 0.149.1 under
    /// `--strict-config`, which rejects a key it does not know, so the set is
    /// verified rather than assumed.
    ///
    /// The base URL goes through verbatim. Codex appends `/responses` to it —
    /// a `base_url` of `http://127.0.0.1:8731/v1` was observed producing
    /// `POST /v1/responses` — so the `/v1` belongs to the provider's own
    /// declared URL and this adapter neither adds nor removes a path segment.
    ///
    /// `env_key` names an environment variable **of the child process**;
    /// its value is what Codex sends as `authorization: Bearer <value>`. With
    /// that variable absent Codex refuses outright ("Missing environment
    /// variable: `…`") rather than falling back to the user's own paid
    /// account — which is why the credential's absence is a refusal here too
    /// rather than a launch that quietly costs the user money.
    ///
    /// `http_headers` is one more override in the same set, present only
    /// when the provider declares headers at all — see `http_headers_table`
    /// below.
    fn direct_provider_launch(
        &self,
        request: &DirectProviderRequest<'_>,
    ) -> Option<DirectProviderPlan> {
        // See `WIRE_API`: `chat` is gone in 0.149.1, so `openai-responses` is
        // the only protocol Codex can be pointed at. Nothing is translated.
        if request.protocol != WireProtocol::OpenAiResponses {
            return None;
        }

        // The provider's name is interpolated into a *dotted TOML path*, so
        // `.` would be a separator rather than a character to escape.
        // `profile::resolve` has already refused anything outside
        // `[A-Za-z0-9_-]` before this request was built — see
        // `super::unsafe_provider_name_char` for why that check lives there
        // and not here.
        let id = request.provider_name;

        let mut args: Vec<OsString> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        let mut override_arg = |key: String, value: &str| {
            args.push(OsString::from("-c"));
            let mut pair = OsString::from(&key);
            pair.push("=");
            pair.push(value);
            args.push(pair);
            keys.push(key);
        };

        // A fixed, deterministic order: the same profile always composes the
        // same argv, so a launch is reproducible and a test can assert on it.
        override_arg("model_provider".to_owned(), id);
        override_arg(format!("model_providers.{id}.name"), request.provider_name);
        override_arg(format!("model_providers.{id}.base_url"), request.base_url);
        override_arg(format!("model_providers.{id}.wire_api"), WIRE_API);
        if let Some(table) = http_headers_table(request.headers) {
            override_arg(format!("model_providers.{id}.http_headers"), &table);
        }
        if let Some(var) = request.credential_var {
            override_arg(format!("model_providers.{id}.env_key"), var);
        }
        if let Some(model) = request.model {
            override_arg("model".to_owned(), model);
        }

        Some(DirectProviderPlan {
            args,
            // Codex needs no environment of its own: everything but the
            // credential is an override, and the credential's destination is
            // the variable `env_key` just named.
            env: Vec::new(),
            credential: request
                .credential_var
                .map(|var| CredentialPlacement::Environment(var.to_owned())),
            // Nothing is written at all — see this method's own doc comment.
            // Codex is the harness that shows a generated configuration file
            // is a *last* resort rather than the shape of the mechanism.
            config: None,
            // Override *keys* and the provider's name only — never a base
            // URL, never a model, and never the value behind `env_key`.
            mechanism: format!("-c overrides: {}", keys.join(", ")),
        })
    }

    fn session_id_source(&self) -> Option<NativeSessionSource> {
        // One rollout per session, each naming its own id, cwd and start
        // time in its first line — the shape `read_session_record` below
        // parses.
        Some(NativeSessionSource::RecordPerSession(
            RecordPerSessionSource {
                home_env: Some("CODEX_HOME"),
                home_default: ".codex",
                subdirectory: "sessions",
                file_prefix: "rollout-",
                file_extension: "jsonl",
            },
        ))
    }

    /// Codex reads hooks from exactly one place — `<project>/.codex/hooks.json`
    /// — with no `--settings`-equivalent flag to point it elsewhere. `args`
    /// is therefore empty: Codex finds the file itself, once it exists.
    ///
    /// That path is inside the user's own repository, so
    /// [`mod@crate::session::select`] must never write it without the user's
    /// explicit consent — see [`HookDestination::ProjectLocal`].
    fn hook_installation(&self, report: &HookCommand) -> Option<HookInstallation> {
        Some(HookInstallation {
            file_name: "hooks.json",
            contents: super::hooks_document(REPORTED_EVENTS, report, HOOK_TIMEOUT_SECONDS),
            args: Invocation::bare(),
            events: REPORTED_EVENTS,
            destination: HookDestination::ProjectLocal {
                relative_path: ".codex/hooks.json",
            },
        })
    }

    /// Read a Codex rollout header.
    ///
    /// Evidence, all read from Codex 0.149.0 across the 555 real rollout
    /// files in `~/.codex/sessions/` on the development machine, on
    /// 2026-08-25:
    ///
    /// - Every rollout's first line is a JSON object with
    ///   `"type":"session_meta"` — 555 of 555.
    /// - `payload.id` is present in all 555 and always equals the UUID in the
    ///   file name. `payload.session_id` is present in only 527 of 555. This
    ///   reads `id`, never `session_id`.
    /// - `payload.cwd` is present in all 555; `payload.timestamp` is an
    ///   RFC3339 UTC instant (`"2026-06-02T20:14:47.633Z"`), present in every
    ///   interactive record.
    /// - `payload.originator` is one of `codex-tui` (241), `Codex Desktop`
    ///   (229), `codex_exec` (81), `codex_work_desktop` (4).
    /// - `payload.parent_thread_id` marks a subagent thread (173 rollouts
    ///   have it); a subagent's `cwd` is the same as its parent's, so `cwd`
    ///   alone cannot tell them apart.
    /// - `originator == "codex-tui"` with `parent_thread_id` absent or null
    ///   selects exactly the 70 real interactive CLI sessions in the 555
    ///   files, zero counterexamples — all 70 also carry `source == "cli"`,
    ///   which corroborates but is deliberately not required, so an
    ///   unrelated Codex update to `source` cannot break this rule.
    ///   `forked_from_id` is not treated as disqualifying: every one of its
    ///   128 occurrences is already excluded by the rule above.
    fn read_session_record(&self, header: &str) -> Option<NativeSessionRecord> {
        let value: serde_json::Value = serde_json::from_str(header).ok()?;
        if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
            return None;
        }
        let payload = value.get("payload")?;

        let id = payload.get("id")?.as_str()?.to_owned();
        let cwd = std::path::PathBuf::from(payload.get("cwd")?.as_str()?);
        let started_at = parse_rfc3339_utc(payload.get("timestamp")?.as_str()?)?;

        let originator = payload
            .get("originator")
            .and_then(serde_json::Value::as_str);
        let has_parent_thread = payload
            .get("parent_thread_id")
            .is_some_and(|value| !value.is_null());
        let kind = if originator == Some("codex-tui") && !has_parent_thread {
            NativeSessionKind::Interactive
        } else {
            NativeSessionKind::Other
        };

        Some(NativeSessionRecord {
            id,
            cwd,
            started_at,
            kind,
        })
    }

    fn official_model_support(&self) -> OfficialModelSupport {
        OfficialModelSupport {
            native_families: Declared::verified(
                NATIVE_FAMILIES,
                "Codex 0.149.1 wrote `\"gpt-5.5\"` and `\"gpt-5.6-sol\"` into the \
                 `[tui.model_availability_nux]` table of its own `~/.codex/config.toml`, \
                 read 2026-08-27 — the harness's own record of which models it offered",
            ),
            supported_models: Declared::verified(
                SUPPORTED_MODELS,
                "`codex --help` (codex-cli 0.149.1, read 2026-08-27) gives `-c model=\"o3\"` \
                 as its own configuration-override example",
            ),
        }
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::OpenAi,
                "`codex login --help` documents authenticating with OPENAI credentials",
            ),
            hooks: Declared::verified(
                Hooks {
                    mechanism: "a `.codex/hooks.json` inside the project, reviewed and \
                                trusted per project by content hash before it runs",
                    verified_events: HOOK_EVENTS,
                },
                "Codex 0.149.1's hook review screen enumerates these eleven events with \
                 descriptions when a project's `.codex/hooks.json` is first seen; it \
                 offers \"Review hooks\" / \"Trust all and continue\" / \"Continue \
                 without trusting (hooks won't run)\", and records the result as \
                 `[hooks.state.\"<path>:<event>:0:0\"]` in `config.toml`",
            ),
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "$CODEX_HOME/sessions/<yyyy>/<mm>/<dd>/rollout-<timestamp>-<uuid>.jsonl",
                },
                "a real Codex installation writes session rollouts under that path with the \
                 session UUID in the file name, and `codex resume` accepts that UUID",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`codex --help`: the `apply` subcommand applies \"the latest diff produced \
                     by Codex agent\" to the working tree",
                ),
                shell_access: Declared::verified(
                    true,
                    "`codex --help`: `-s/--sandbox` selects \"the sandbox policy to use when \
                     executing model-generated shell commands\"",
                ),
                // Codex 0.149.0's `--help` documents `--search` for web search
                // but names no browser-control capability. Absent evidence is
                // not evidence of absence.
                browser_use: Declared::Unverified,
                mcp: Declared::verified(
                    true,
                    "`codex --help`: an `mcp` subcommand manages external MCP servers, and \
                     `mcp-server` runs Codex as one",
                ),
                subagents: Declared::verified(
                    true,
                    "`codex --help`: an `agents` subcommand browses agent sessions, and its \
                     hook state records subagent_start/subagent_stop events",
                ),
            },
            backends: Backends {
                protocols: Declared::verified(
                    PROTOCOLS,
                    "`codex --help` under `--search`: \"the native Responses `web_search` tool \
                     is available to the model\"",
                ),
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`codex --help`: `-m/--model <MODEL>`, and `-c model=\"o3\"` is given as an \
                     explicit example of a config override",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`codex --help`: `-c <key=value>`, `--oss`, `--local-provider`, and \
                     `-p/--profile` (\"Layer $CODEX_HOME/<name>.config.toml on top of the base \
                     user config\")",
                ),
            },
            approvals: ApprovalModes {
                automatic_review: Declared::verified(
                    ApprovalMode {
                        args: &["--approve-for-me"],
                        description: "Route approval requests through automatic review using \
                                       the workspace-write sandbox",
                    },
                    "`codex --help`: `--approve-for-me` — \"Route approval requests through \
                     automatic review using the workspace-write sandbox\"",
                ),
                bypass: Declared::verified(
                    ApprovalMode {
                        args: &["--dangerously-bypass-approvals-and-sandbox"],
                        description: "Skip all confirmation prompts and execute commands \
                                       without sandboxing. EXTREMELY DANGEROUS",
                    },
                    "`codex --help`: `--dangerously-bypass-approvals-and-sandbox` — \"Skip all \
                     confirmation prompts and execute commands without sandboxing. EXTREMELY \
                     DANGEROUS\"; `-a never` (`--ask-for-approval <on-request|never>`) reaches \
                     the same effect",
                ),
                sandbox: Declared::verified(
                    SandboxSelector {
                        flag: "--sandbox",
                        values: &["read-only", "workspace-write", "danger-full-access"],
                    },
                    "`codex --help`: `-s/--sandbox <read-only|workspace-write|danger-full-access>` \
                     selects the sandbox policy for model-generated shell commands",
                ),
            },
            communication_style: COMMUNICATION_STYLE,
        }
    }
}

/// Parse an RFC3339 UTC instant, e.g. `"2026-06-02T20:14:47.633Z"` — the
/// exact shape `payload.timestamp` takes in every Codex rollout header
/// sampled above. Accepts a `Z` suffix or a numeric `+HH:MM`/`-HH:MM` offset;
/// rejects anything else, including a bare instant with no offset at all.
///
/// This carries no Codex-specific knowledge — it is generic RFC3339 parsing
/// — but lives here rather than in `session::native_id` because Codex is the
/// only caller: no RFC3339-capable crate is a workspace dependency, and
/// promoting this out to somewhere more general is only worth doing once a
/// second harness needs it too.
fn parse_rfc3339_utc(text: &str) -> Option<std::time::SystemTime> {
    let year: i64 = text.get(0..4)?.parse().ok()?;
    if text.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    let month: u32 = text.get(5..7)?.parse().ok()?;
    if text.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    let day: u32 = text.get(8..10)?.parse().ok()?;
    match text.as_bytes().get(10) {
        Some(&b'T') | Some(&b't') => {}
        _ => return None,
    }
    let hour: u32 = text.get(11..13)?.parse().ok()?;
    if text.as_bytes().get(13) != Some(&b':') {
        return None;
    }
    let minute: u32 = text.get(14..16)?.parse().ok()?;
    if text.as_bytes().get(16) != Some(&b':') {
        return None;
    }
    let second: u32 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut rest = text.get(19..)?;
    let mut nanos: u32 = 0;
    if let Some(stripped) = rest.strip_prefix('.') {
        let digit_count = stripped
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(stripped.len());
        if digit_count == 0 {
            return None;
        }
        let (frac, remainder) = stripped.split_at(digit_count);
        let mut nine_digits = frac.to_owned();
        nine_digits.truncate(9);
        while nine_digits.len() < 9 {
            nine_digits.push('0');
        }
        nanos = nine_digits.parse().ok()?;
        rest = remainder;
    }

    let offset_seconds: i64 = if rest.eq_ignore_ascii_case("z") {
        0
    } else if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) {
        if rest.as_bytes()[3] != b':' {
            return None;
        }
        let sign: i64 = if rest.starts_with('-') { -1 } else { 1 };
        let offset_hours: i64 = rest.get(1..3)?.parse().ok()?;
        let offset_minutes: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (offset_hours * 3600 + offset_minutes * 60)
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day);
    let seconds_of_day = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    let total_seconds = u64::try_from(days * 86_400 + seconds_of_day - offset_seconds).ok()?;

    Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::new(total_seconds, nanos))
}

/// Days since the Unix epoch for a Gregorian civil date. Howard Hinnant's
/// `days_from_civil` — <http://howardhinnant.github.io/date_algorithms.html>
/// — valid for every date [`parse_rfc3339_utc`] can produce (`month` and
/// `day` are already range-checked by its caller).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400; // [0, 399]
    let month = i64::from(month);
    let day = i64::from(day);
    let month_index = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let day_of_year = (153 * month_index + 2) / 5 + day - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // [0, 146096]
    era * 146097 + day_of_era - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_fractional_second_zulu_form_codex_actually_writes() {
        let parsed = parse_rfc3339_utc("2026-06-02T20:14:47.633Z").expect("a valid instant");
        let seconds = parsed
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs();
        // 2026-06-02T20:14:47Z, independently computed.
        assert_eq!(seconds, 1_780_431_287);
    }

    #[test]
    fn rejects_an_instant_with_no_offset() {
        assert!(parse_rfc3339_utc("2026-06-02T20:14:47").is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_rfc3339_utc("not a timestamp").is_none());
        assert!(parse_rfc3339_utc("").is_none());
    }

    // --- lifecycle hooks -------------------------------------------------

    fn hook_command() -> HookCommand {
        HookCommand::new(
            "/opt/glasshouse/glasshouse",
            "0123456789abcdef0123456789abcdef",
            "/state/sessions/0123456789abcdef0123456789abcdef",
            "/work/project",
            "/state",
            "/config",
        )
    }

    #[test]
    fn codex_declares_a_project_local_destination() {
        // Codex has no `--settings`-equivalent flag, so its installation must
        // ask to be written inside the project itself, at the one path Codex
        // actually reads.
        let installation = Codex
            .hook_installation(&hook_command())
            .expect("an installation");
        assert_eq!(
            installation.destination,
            HookDestination::ProjectLocal {
                relative_path: ".codex/hooks.json",
            }
        );
        assert!(
            installation.args.is_bare(),
            "Codex finds the file itself; no argument should point it there"
        );
    }

    #[test]
    fn a_codex_hook_declares_a_timeout_codex_will_not_clamp() {
        // Codex clamps a declared timeout to 3s and announces it when it
        // does. Every declared timeout must already be at or under that, so
        // a real installation never produces the warning.
        let installation = Codex
            .hook_installation(&hook_command())
            .expect("an installation");
        let parsed: serde_json::Value = serde_json::from_str(&installation.contents)
            .unwrap_or_else(|err| panic!("not valid JSON: {err}\n{}", installation.contents));
        let hooks = parsed["hooks"].as_object().expect("a hooks object");
        assert!(!hooks.is_empty());
        for (event, entries) in hooks {
            let timeout = entries[0]["hooks"][0]["timeout"]
                .as_u64()
                .unwrap_or_else(|| panic!("{event} declares no numeric timeout"));
            assert!(
                timeout <= 3,
                "{event} declares timeout {timeout}, which Codex would clamp to 3"
            );
        }
    }

    #[test]
    fn codex_reports_exactly_the_seven_session_level_and_compaction_events() {
        // Not the remaining per-tool events (`PreToolUse`/`PostToolUse`/
        // `SubagentStart`/`SubagentStop`): those fire many times per turn and
        // say nothing about a session's state. `PreCompact`/`PostCompact` are
        // included even though compaction is not a `SessionLifecycle` state —
        // see the doc comment on `REPORTED_EVENTS`.
        let installation = Codex
            .hook_installation(&hook_command())
            .expect("an installation");
        let mut events: Vec<&str> = installation.events.to_vec();
        events.sort_unstable();
        let mut expected = vec![
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            "Stop",
            "SessionEnd",
            "PreCompact",
            "PostCompact",
        ];
        expected.sort_unstable();
        assert_eq!(events, expected);
    }

    #[test]
    fn the_generated_document_registers_a_command_for_both_compaction_events() {
        // §35: a caller every test bypasses is not a caller. This checks the
        // generated JSON itself, not just the declared event list, so a
        // regression that declared the events without emitting a runnable
        // command for them would still fail here.
        let installation = Codex
            .hook_installation(&hook_command())
            .expect("an installation");
        let parsed: serde_json::Value = serde_json::from_str(&installation.contents)
            .unwrap_or_else(|err| panic!("not valid JSON: {err}\n{}", installation.contents));
        let hooks = parsed["hooks"].as_object().expect("a hooks object");
        for event in ["PreCompact", "PostCompact"] {
            let entries = hooks
                .get(event)
                .unwrap_or_else(|| panic!("{event} has no entry in the generated document"))
                .as_array()
                .unwrap_or_else(|| panic!("{event}'s entry is not an array"));
            assert!(
                !entries.is_empty(),
                "{event} has no matcher entries in the generated document"
            );
            let command = entries[0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_else(|| panic!("{event} declares no command string"));
            assert!(!command.is_empty(), "{event} declares an empty command");
            let timeout = entries[0]["hooks"][0]["timeout"]
                .as_u64()
                .unwrap_or_else(|| panic!("{event} declares no numeric timeout"));
            assert_eq!(
                timeout,
                u64::from(HOOK_TIMEOUT_SECONDS),
                "{event} does not declare the shared hook timeout"
            );
        }
    }
}
