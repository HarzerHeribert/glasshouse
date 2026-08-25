//! Antigravity.
//!
//! Read from Antigravity CLI 1.1.20 as installed on the development machine on
//! 2026-08-25 — `agy --help` and the package that installed it.
//!
//! # The name
//!
//! Until this install existed, Glasshouse searched `PATH` for `antigravity`
//! and would never have found a real one: the published package ships a
//! binary called `antigravity` but puts it on `PATH` as **`agy`**. Both names
//! are searched now, `agy` first, because that is what an install actually
//! produces. This is the whole argument for deriving adapter declarations
//! from real binaries rather than from plausible-sounding recollection — the
//! previous single-name guess was carefully reasoned and simply wrong.
//!
//! # The state root
//!
//! Conversations live under `~/.gemini/antigravity-cli/`, not
//! `~/.gemini/antigravity/`. An earlier revision of this file, and the
//! evidence ledger it was read from, named the latter — that directory
//! belongs to the *desktop app*, its `conversations/` is permanently empty,
//! and nothing had ever been run against it to say otherwise. The CLI's own
//! root, confirmed against a signed-in install on 2026-08-25, is
//! `antigravity-cli`. This is the fourth declaration in this project derived
//! from an artifact that did not serve the purpose it was cited for.
//!
//! Inside that root, `cache/last_conversations.json` maps each project's
//! absolute path to the conversation UUID Antigravity last opened there —
//! see [`Antigravity::read_last_conversation`]. Reading it is deliberately
//! **not** wired through [`super::HarnessAdapter::session_id_source`] /
//! [`super::HarnessAdapter::read_session_record`]: that pair assumes a
//! harness's session store holds one record per session, self-describing its
//! own id/cwd/timestamp in a header `session::native_id::discover` opens and
//! parses. Antigravity's records are `conversations/<uuid>.db` — SQLite
//! databases that must never be opened (see the module's security note) —
//! and the identifier is not in any record's own contents at all; it is an
//! entry in one shared index, keyed by project path, that has to be read as
//! a whole rather than discovered by walking and filtering file names.
//! That is a genuinely different shape, so `session_id_source` is left
//! undeclared here rather than populated with directory/extension values
//! that would compile but would send `discover` to open every conversation
//! database on the box the moment it runs. Wiring
//! [`Antigravity::read_last_conversation`] into core is an interface
//! decision for whoever owns `harness/mod.rs` and `session/`, not this file.

use std::path::Path;

use super::{
    ApprovalMode, ApprovalModes, BackendSelection, Backends, Capabilities, Declared,
    HarnessAdapter, HarnessDescription, Invocation, ModelOverride, SandboxSelector, SessionIds,
    Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Antigravity;

const MODEL_OVERRIDE: &[ModelOverride] = &[ModelOverride::CommandLine("--model")];

const BACKEND_SELECTION: &[BackendSelection] = &[BackendSelection::CommandLineArguments(
    "--model and --project select what a session runs against",
)];

impl HarnessAdapter for Antigravity {
    fn id(&self) -> IntegrationId {
        IntegrationId::Antigravity
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        // `agy` first: it is the name the published package links onto
        // `PATH`. `antigravity` second: it is the name of the binary inside
        // that package, so an install that copies it directly, or a future
        // package that links it under its own name, still resolves.
        //
        // Deliberately no shorter alias. `ag` is the-silver-searcher on a
        // great many machines, and a confident wrong detection is worse than
        // a missed one — the user can always configure an explicit path.
        &["agy", "antigravity"]
    }

    fn start(&self) -> Invocation {
        // `agy` with no arguments starts an interactive CLI session; `--print`
        // is the non-interactive mode.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `agy --help`: `--conversation  Resume a previous conversation by ID`.
        //
        // `agy --conversation <unknown-uuid>` does **not** fail: it prints
        // `warning: conversation "<id>" not found` to stderr and starts a
        // brand new conversation anyway, exiting 0. Unlike Codex, which
        // refuses an unknown session outright, Antigravity gives Glasshouse
        // no way to detect a bad resume after the fact — the warning is not
        // a refusal and nothing here may treat it as one. The only safety
        // available is upstream of this function: `native_session` must
        // always be an identifier Glasshouse itself recorded (from
        // `Antigravity::read_last_conversation`), never one it invented,
        // guessed, or received from anywhere else.
        Some(Invocation::of(["--conversation", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Google,
                "the published Antigravity CLI package is Google's, distributed from \
                 antigravity.google",
            ),
            // A signed-in `agy --help` was read in full: subcommands are
            // `agent(s)`, `changelog`, `help`, `install`, `mcp`,
            // `mic-serve`, `models`, `plugin(s)`, `update`. None of them, and
            // no flag alongside them, is a hook, event, or notification
            // mechanism. Unverified here means genuinely unavailable, not
            // merely unchecked — being signed in is exactly what closes that
            // gap.
            hooks: Declared::Unverified,
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "~/.gemini/antigravity-cli/cache/last_conversations.json, keyed \
                             by absolute project path",
                },
                "on a signed-in Antigravity CLI 1.1.20 install, \
                 ~/.gemini/antigravity-cli/cache/last_conversations.json is a flat \
                 `{ \"<absolute project path>\": \"<uuid>\" }` object, and each conversation \
                 is a `conversations/<uuid>.db` SQLite database matching that UUID",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`agy --help`: `--mode` selects an execution mode, one of which is \
                     `accept-edits`",
                ),
                shell_access: Declared::verified(
                    true,
                    "`agy --help`: `--sandbox` — \"Run in a sandbox with terminal restrictions \
                     enabled\"",
                ),
                browser_use: Declared::Unverified,
                mcp: Declared::verified(
                    true,
                    "`agy --help`: an `mcp` subcommand manages MCP servers",
                ),
                // `--agent` selects an agent for the session and `agents`
                // lists them; neither shows that a session can *spawn* one,
                // which is what the capability means.
                subagents: Declared::Unverified,
            },
            backends: Backends {
                protocols: Declared::Unverified,
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`agy --help`: `--model  Model for the current CLI session`",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`agy --help` documents `--model` and `--project` as per-session selectors; \
                     no environment or configuration mechanism is documented there",
                ),
            },
            approvals: ApprovalModes {
                // `agy --help` documents no classifier-style mode.
                automatic_review: Declared::Unverified,
                bypass: Declared::verified(
                    ApprovalMode {
                        args: &["--dangerously-skip-permissions"],
                        description: "Auto-approve all tool permission requests without \
                                       prompting",
                    },
                    "`agy --help`: `--dangerously-skip-permissions` — \"Auto-approve all tool \
                     permission requests without prompting\"",
                ),
                sandbox: Declared::verified(
                    SandboxSelector {
                        flag: "--sandbox",
                        values: &[],
                    },
                    "`agy --help`: `--sandbox` — \"Run in a sandbox with terminal restrictions \
                     enabled\"",
                ),
            },
            communication_style: Declared::Unverified,
        }
    }
}

impl Antigravity {
    /// Read the conversation UUID Antigravity itself associated with
    /// `project_root`, from the text of its own index —
    /// `~/.gemini/antigravity-cli/cache/last_conversations.json`.
    ///
    /// A pure function from text to an identifier, matching
    /// [`HarnessAdapter::read_session_record`]'s precedent: core resolves the
    /// path and reads the file, this function only ever looks at the bytes
    /// it is handed. It never opens a conversation database, never reads any
    /// value from the index but the one matching `project_root`, and never
    /// returns anything else the index might contain (an account email is
    /// not part of this file's shape, but the same discipline would apply if
    /// it ever were).
    ///
    /// The index's keys are matched against `project_root` through
    /// [`crate::platform::paths::same_path`] — component-wise, not with
    /// `==` — so a key differing only by a `.` component, a trailing
    /// separator, or (on Windows) letter case still matches the project it
    /// names.
    pub fn read_last_conversation(index_json: &str, project_root: &Path) -> Option<String> {
        let index: serde_json::Value = serde_json::from_str(index_json).ok()?;
        let entries = index.as_object()?;
        entries.iter().find_map(|(path, id)| {
            crate::platform::paths::same_path(Path::new(path), project_root)
                .then(|| id.as_str())
                .flatten()
                .map(str::to_owned)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // --- session identity ------------------------------------------------

    #[test]
    fn antigravity_resumes_only_an_identifier_glasshouse_recorded() {
        // `agy --conversation <unknown-uuid>` does not fail: it prints
        // `warning: conversation "<id>" not found` and starts a fresh
        // conversation anyway, exiting 0. That warning is not a refusal
        // Glasshouse can detect, so the only guarantee available is that the
        // resume invocation is built from nothing but the identifier the
        // caller supplies — no constant fallback, no environment, no
        // rediscovery. This pins exactly that.
        let recorded_id = "6cc20c51-e7d6-4b94-a000-4db47b58797c";
        let invocation = Antigravity
            .resume(recorded_id)
            .expect("Antigravity resumes");
        let args: Vec<String> = invocation
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec!["--conversation".to_owned(), recorded_id.to_owned()]
        );
    }

    #[test]
    fn antigravity_declares_no_hook_mechanism() {
        // A signed-in `agy --help` was read in full and names no hook,
        // event, or notification mechanism anywhere in it — see the comment
        // on `hooks` in `describe`.
        let description = Antigravity.describe();
        assert!(matches!(description.hooks, Declared::Unverified));
    }

    // --- the conversation index -------------------------------------------

    fn index_with(entries: &[(&str, &str)]) -> String {
        let body = entries
            .iter()
            .map(|(path, id)| format!("{path:?}:{id:?}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{body}}}")
    }

    #[test]
    fn the_conversation_index_yields_the_identifier_for_this_project() {
        let project = PathBuf::from("/Users/example/projects/glasshouse");
        let index = index_with(&[(
            "/Users/example/projects/glasshouse",
            "6cc20c51-e7d6-4b94-a000-4db47b58797c",
        )]);
        assert_eq!(
            Antigravity::read_last_conversation(&index, &project),
            Some("6cc20c51-e7d6-4b94-a000-4db47b58797c".to_owned())
        );
    }

    #[test]
    fn a_project_with_no_entry_yields_no_identifier() {
        let project = PathBuf::from("/Users/example/projects/glasshouse");
        let index = index_with(&[]);
        assert_eq!(Antigravity::read_last_conversation(&index, &project), None);
    }

    #[test]
    fn another_projects_entry_is_never_returned() {
        let project = PathBuf::from("/Users/example/projects/glasshouse");
        let index = index_with(&[(
            "/Users/example/projects/other",
            "aaaaaaaa-0000-4000-8000-000000000000",
        )]);
        assert_eq!(Antigravity::read_last_conversation(&index, &project), None);
    }

    #[test]
    fn the_index_is_matched_canonically_not_lexically() {
        let index = index_with(&[(
            "/Users/example/projects/glasshouse",
            "6cc20c51-e7d6-4b94-a000-4db47b58797c",
        )]);
        // A `.` component and a trailing separator: not the exact bytes the
        // index carries, but the same path.
        let project = PathBuf::from("/Users/example/projects/./glasshouse/");
        assert_eq!(
            Antigravity::read_last_conversation(&index, &project),
            Some("6cc20c51-e7d6-4b94-a000-4db47b58797c".to_owned())
        );
    }

    #[test]
    fn a_malformed_index_yields_no_identifier_rather_than_an_error() {
        let project = PathBuf::from("/Users/example/projects/glasshouse");
        assert_eq!(
            Antigravity::read_last_conversation("not json", &project),
            None
        );
        assert_eq!(Antigravity::read_last_conversation("", &project), None);
        // Valid JSON, but not the flat object shape the index actually is.
        assert_eq!(Antigravity::read_last_conversation("[]", &project), None);
        assert_eq!(Antigravity::read_last_conversation("42", &project), None);
    }

    #[test]
    fn nothing_but_the_identifier_is_read_from_the_index() {
        // The conversation database itself is never opened by this
        // function — it never takes a database path or touches a
        // filesystem at all, only the index text it is handed. This also
        // pins that unrelated content elsewhere in the index (another
        // project's id, a non-string value) never leaks into the result for
        // a project that does have a clean match.
        let project = PathBuf::from("/Users/example/projects/glasshouse");
        let index = format!(
            "{{{},{},{}}}",
            r#""/Users/example/projects/glasshouse":"6cc20c51-e7d6-4b94-a000-4db47b58797c""#,
            r#""/Users/example/projects/other":{"nested":"should never surface"}"#,
            r#""unrelated_metadata":"should never surface""#
        );
        assert_eq!(
            Antigravity::read_last_conversation(&index, &project),
            Some("6cc20c51-e7d6-4b94-a000-4db47b58797c".to_owned())
        );
    }
}
