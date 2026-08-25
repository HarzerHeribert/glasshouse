//! Codex.
//!
//! Read from Codex 0.149.0 as installed on the development machine on
//! 2026-08-25 — `codex --help`, `codex resume --help`, `codex login --help`,
//! the hook state it records in its own configuration, and the session
//! rollouts it writes.

use super::{
    BackendSelection, Backends, Capabilities, Declared, HarnessAdapter, HarnessDescription, Hooks,
    Invocation, ModelOverride, NativeSessionKind, NativeSessionRecord, NativeSessionSource,
    SessionIds, Vendor, WireProtocol,
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
const PROTOCOLS: &[WireProtocol] = &[WireProtocol::OpenAiResponses];

const MODEL_OVERRIDE: &[ModelOverride] = &[
    ModelOverride::CommandLine("--model"),
    ModelOverride::Configuration("-c model=<id>"),
];

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

    fn session_id_source(&self) -> Option<NativeSessionSource> {
        Some(NativeSessionSource {
            home_env: "CODEX_HOME",
            home_default: ".codex",
            subdirectory: "sessions",
            file_prefix: "rollout-",
            file_extension: "jsonl",
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
            // Codex 0.149.0's `--help` documents no output-style, persona, or
            // tone mechanism. The capability map anticipates "Codex
            // personalities"; this installation does not expose one, so
            // Glasshouse records that it has not seen one rather than
            // implying the map's example is present.
            communication_style: Declared::Unverified,
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
}
