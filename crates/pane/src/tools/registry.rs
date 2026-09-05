//! What a tool is — the registry's own schema, which
//! `docs/product/pane/runtime-contract.md` §7 leaves to this sub-phase and
//! decides nothing about.
//!
//! The invariant this module exists for: **a tool's purity is declared at its
//! definition and there is no expression that omits it.** [`Purity`] has no
//! `Default`, [`Tool`] has no `Default` and no builder, and
//! [`Tool::declare`] takes purity as a positional argument, so a declaration
//! that leaves it out is a compile error rather than a silent `false`.
//! `runtime-contract.md` §4 is why it matters: a resumed handle
//! re-materialises by re-running a recorded *pure* call and comparing
//! SHA-256, so a tool wrongly declared pure would silently re-run something
//! with an effect.
//!
//! The second invariant is an absence: **a tool that needs the network is
//! not registered at all** (`sandbox-grants.md` §4.1). It is absent rather
//! than present-and-failing, and [`NEVER_REGISTERED`] names the absences so
//! a test can assert them positively instead of asserting that a list is
//! short.

use std::fmt;

/// Whether re-running a call reproduces its result without changing the
/// world.
///
/// Two variants and no third: `runtime-contract.md` §4 asks one yes/no
/// question of a recorded call, and a "probably" would be answered as `Pure`
/// by the only consumer there is.
///
/// **Deliberately not `Default`, not `Option`, and not a `bool`.** Each of
/// those would give a declaration a way to say nothing and be read as
/// something. The `bool` is the worst of the three, because `false` and
/// `true` are both plausible defaults and the reader of a call site cannot
/// tell which was meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purity {
    /// Re-running reproduces the same bytes and changes nothing. Only a
    /// `Pure` call may be re-run to re-materialise a stale handle.
    Pure,
    /// Running it may change the world. Never re-run on resume.
    Effectful,
}

impl Purity {
    pub fn as_str(self) -> &'static str {
        match self {
            Purity::Pure => "pure",
            Purity::Effectful => "effectful",
        }
    }

    /// Whether a recorded call of this kind may be re-run on resume.
    pub fn may_rematerialise(self) -> bool {
        matches!(self, Purity::Pure)
    }
}

impl fmt::Display for Purity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one argument is, which decides **which** of `sandbox-grants.md` §2's
/// two questions it is asked.
///
/// The kind is the whole of the type system here, and that is the point:
/// §2's own warning is that conflating a filesystem grant with argv
/// admission inverts the model, so an argument declares which question it
/// answers and [`crate::tools::invoke`] has no branch that can ask the other
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// A filesystem path. Goes through `Profile::check`, and the **resolved**
    /// path it returns is what reaches the child.
    Path,
    /// An opaque string handed to the child as one argv element. It is never
    /// parsed, never expanded, and never spliced into a command line, so
    /// there is nothing in it for a shell to interpret.
    Pattern,
    /// A whole command line. Goes through `Profile::admits_command` and
    /// through nothing else — it grants no file access whatsoever (§2).
    CommandLine,
}

/// One declared argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arg {
    name: &'static str,
    kind: ArgKind,
    /// `false` only where [`Argv`] has a stated substitute for the missing
    /// value; there is no argument that is optional and then absent.
    required: bool,
}

impl Arg {
    pub const fn required(name: &'static str, kind: ArgKind) -> Self {
        Self {
            name,
            kind,
            required: true,
        }
    }

    /// An argument the project root stands in for when it is not given. Only
    /// [`ArgKind::Path`] has such a substitute, because the project root is
    /// the one path every profile grants (`sandbox-grants.md` §1.3).
    pub const fn rooted(name: &'static str) -> Self {
        Self {
            name,
            kind: ArgKind::Path,
            required: false,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn kind(&self) -> ArgKind {
        self.kind
    }

    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// How a tool's **checked** arguments become the child's argv.
///
/// It is part of the declaration rather than a branch in the invoker so that
/// one place says everything about a tool. Every variant places the checked
/// values positionally and quotes nothing: the child is spawned through
/// `execvp`, never through a shell, so there is no string for an argument to
/// escape out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Argv {
    /// `<exe> -- <path>`.
    ReadPath,
    /// `<exe> -r -n -e <pattern> -- <path>`.
    GrepIn,
    /// `<exe> <path> -name <pattern>`.
    FindNamed,
    /// `<exe> -c <command line>`. The one variant whose argument was
    /// admitted by `Profile::admits_command` rather than by `Profile::check`.
    ShellCommand,
}

/// One tool: its name, its arguments, the executable it runs, how the two
/// become an argv, and its declared purity.
///
/// Every field is private and there is no constructor but [`Tool::declare`],
/// which takes all five. That is the mechanism behind this module's first
/// invariant — see the module header — and it is structural rather than a
/// promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    name: &'static str,
    executable: &'static str,
    args: &'static [Arg],
    argv: Argv,
    purity: Purity,
}

impl Tool {
    /// Declares one tool. `purity` is positional and has no default:
    ///
    /// ```
    /// use pane::tools::registry::{Argv, Purity, Tool};
    /// let tool = Tool::declare("read", "cat", &[], Argv::ReadPath, Purity::Pure);
    /// assert_eq!(tool.purity(), Purity::Pure);
    /// ```
    ///
    /// Omitting it does not compile, which is the answer to "what happens to
    /// a tool that declares no purity":
    ///
    /// ```compile_fail
    /// use pane::tools::registry::{Argv, Tool};
    /// let _ = Tool::declare("read", "cat", &[], Argv::ReadPath);
    /// ```
    ///
    /// and neither does reaching for a default:
    ///
    /// ```compile_fail
    /// use pane::tools::registry::Purity;
    /// let _: Purity = Default::default();
    /// ```
    pub const fn declare(
        name: &'static str,
        executable: &'static str,
        args: &'static [Arg],
        argv: Argv,
        purity: Purity,
    ) -> Self {
        Self {
            name,
            executable,
            args,
            argv,
            purity,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The program this tool runs, as a name to resolve rather than a path.
    /// [`crate::tools::invoke`] resolves it and grants exec on the resolved
    /// binary — the 61D exec-roots ruling — so a hard-coded path here would
    /// be a second, staler answer to a question that already has one.
    pub fn executable(&self) -> &'static str {
        self.executable
    }

    pub fn args(&self) -> &'static [Arg] {
        self.args
    }

    pub fn argv(&self) -> Argv {
        self.argv
    }

    pub fn purity(&self) -> Purity {
        self.purity
    }
}

/// `read({ path })` — `runtime-contract.md` §6's own spelling.
///
/// Pure: `cat` of a path reads bytes and writes nothing.
const READ: Tool = Tool::declare(
    "read",
    "cat",
    &[Arg::required("path", ArgKind::Path)],
    Argv::ReadPath,
    Purity::Pure,
);

/// `glob({ pattern, path? })` — names matching `pattern` beneath `path`,
/// which defaults to the project root.
///
/// Pure: `find -name` reads directory entries and writes nothing.
const GLOB: Tool = Tool::declare(
    "glob",
    "find",
    &[
        Arg::required("pattern", ArgKind::Pattern),
        Arg::rooted("path"),
    ],
    Argv::FindNamed,
    Purity::Pure,
);

/// `grep({ pattern, path? })` — §6's own spelling, with `glob` narrowed to a
/// path because a second glob language here would be a second answer to
/// `sandbox-grants.md` §2's pattern question.
///
/// Pure: `grep -r` reads and writes nothing.
const GREP: Tool = Tool::declare(
    "grep",
    "grep",
    &[
        Arg::required("pattern", ArgKind::Pattern),
        Arg::rooted("path"),
    ],
    Argv::GrepIn,
    Purity::Pure,
);

/// `bash({ command })` — a command line, admitted by
/// `Profile::admits_command` and by nothing else.
///
/// **Effectful, and it is the reason the declaration is explicit.** Nothing
/// about `bash -c` says whether the line it runs has an effect; the answer
/// cannot be inferred from the tool, the argument or the result, so it is
/// declared once here and a resumed handle is never re-materialised from it.
///
/// **`bash`, not `sh`, because the sandbox grants exec on one resolved
/// binary.** On macOS `/bin/sh` is a shim that re-execs `/bin/bash`, and a
/// grant on `/bin/sh` alone refuses that second exec (`sandbox_apply.rs`'s
/// sibling test is the rule working). `/bin/bash` is the binary itself.
const BASH: Tool = Tool::declare(
    "bash",
    "bash",
    &[Arg::required("command", ArgKind::CommandLine)],
    Argv::ShellCommand,
    Purity::Effectful,
);

/// Every registered tool.
///
/// Small on purpose: each entry is a program that gets exec'd inside a
/// sandbox, so the set is the attack surface and it grows by a package, not
/// by a convenience.
pub const ALL: [Tool; 4] = [READ, GLOB, GREP, BASH];

/// Tools that are **absent**, by name, and why.
///
/// `sandbox-grants.md` §4.1 says a network-needing tool is not registered
/// rather than present and failing. An absence is invisible to a test that
/// only reads what is there, so the names are written down and
/// `tests/tools.rs::no_registered_tool_needs_the_network` asserts that none
/// of them — nor any program that would reach a network — appears in
/// [`ALL`].
pub const NEVER_REGISTERED: [&str; 6] = ["webfetch", "websearch", "fetch", "curl", "wget", "http"];

/// Programs that reach a network, checked against every declared
/// [`Tool::executable`]. The companion to [`NEVER_REGISTERED`]: a tool named
/// innocuously that shells out to `curl` is the shape a name-only list
/// misses.
pub const NETWORK_PROGRAMS: [&str; 8] = [
    "curl", "wget", "nc", "netcat", "ssh", "scp", "ftp", "telnet",
];

/// The declaration for `name`, or `None`. An unknown name is not an error
/// here — [`crate::tools::invoke`] turns it into a refusal, which is a value
/// (`sandbox-grants.md` §1.4).
pub fn lookup(name: &str) -> Option<&'static Tool> {
    ALL.iter().find(|tool| tool.name == name)
}

/// Every registered name, for a caller listing what exists.
pub fn names() -> Vec<&'static str> {
    ALL.iter().map(|tool| tool.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_tool_states_a_purity() {
        for tool in ALL {
            assert!(
                matches!(tool.purity(), Purity::Pure | Purity::Effectful),
                "{} declared no purity",
                tool.name()
            );
        }
    }

    #[test]
    fn bash_is_the_only_effectful_tool() {
        let effectful: Vec<_> = ALL
            .iter()
            .filter(|tool| tool.purity() == Purity::Effectful)
            .map(Tool::name)
            .collect();
        assert_eq!(effectful, vec!["bash"]);
    }

    #[test]
    fn only_a_pure_call_may_rematerialise_a_handle() {
        assert!(Purity::Pure.may_rematerialise());
        assert!(!Purity::Effectful.may_rematerialise());
    }

    #[test]
    fn a_command_line_argument_belongs_to_bash_alone() {
        for tool in ALL {
            let has_command_line = tool
                .args()
                .iter()
                .any(|arg| arg.kind() == ArgKind::CommandLine);
            assert_eq!(
                has_command_line,
                tool.name() == "bash",
                "{} asks the wrong one of the two questions",
                tool.name()
            );
        }
    }

    #[test]
    fn an_optional_argument_is_a_path_the_root_stands_in_for() {
        for tool in ALL {
            for arg in tool.args() {
                assert!(
                    arg.is_required() || arg.kind() == ArgKind::Path,
                    "{}({}) is optional with nothing to stand in for it",
                    tool.name(),
                    arg.name()
                );
            }
        }
    }

    #[test]
    fn lookup_answers_none_for_a_name_that_was_never_declared() {
        assert!(lookup("read").is_some());
        assert!(lookup("webfetch").is_none());
        assert!(lookup("Read").is_none());
    }
}
