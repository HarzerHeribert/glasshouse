//! One immutable profile compiled from `.claude/settings.json`'s
//! `permissions`, and the pre-call check that answers every path question
//! from it — map line 2455, specification
//! `docs/product/pane/sandbox-grants.md`.
//!
//! The invariant the whole module exists for: **a profile is built once and
//! can never be widened afterwards.** That is enforced by the type rather
//! than by a comment — [`Profile`] has no public field, no setter, no method
//! taking a mutable receiver and no shared-mutable interior, so there is no
//! expression a later module could write that adds a grant. It also holds no
//! handle to the document it was compiled from, which is why re-reading
//! `.claude/settings.json` mid-session — the widening path §1.5 names, since
//! `.claude/` lives inside the writable project root — is not something a
//! caller can accidentally do.
//!
//! Two questions, answered in that order and never conflated (§2):
//! [`Profile::admits_command`] asks whether a command line may be attempted,
//! and [`Profile::check`] asks what any process may touch. A `Bash` pattern
//! answers the first and contributes nothing to the second.

use crate::contract::ProjectConfig;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// Which half of a file grant a call needs. An editing tool asks both, in
/// whichever order it performs them; there is no combined variant, because a
/// single answer would hide which half was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }

    /// The refusal sentence for a path no grant covers, §5.
    fn only_root_sentence(self) -> &'static str {
        match self {
            Access::Read => "no grant covers this path; the project root is the only readable root",
            Access::Write => {
                "no grant covers this path; the project root is the only writable root"
            }
        }
    }
}

/// A refusal, as a value.
///
/// It is returned, never raised as a question: nothing in this module reads
/// from a terminal and nothing here widens anything (§1.4). `rule`
/// names the *deciding* rule — the `deny` entry that matched, the
/// never-grantable entry that matched, or the absence of any `allow` — so a
/// person reading a transcript can fix the settings file without
/// re-deriving the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDenied {
    pub tool: String,
    /// The path asked for, as it was resolved. For an argv refusal this is
    /// the command line, because that is what was refused.
    pub path: String,
    pub rule: String,
}

impl fmt::Display for PermissionDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PermissionDenied: {}(\"{}\")\n  rule: {}\n  tool: {}",
            self.tool, self.path, self.rule, self.tool
        )
    }
}

impl std::error::Error for PermissionDenied {}

/// One filesystem rule, keeping the pattern as written so a refusal can quote
/// it back.
#[derive(Debug, Clone)]
struct PathRule {
    written: String,
    glob: Vec<String>,
    read: bool,
    write: bool,
}

/// One entry of §4's never-grantable set, expressed as a resolved subtree.
#[derive(Debug, Clone)]
struct NeverRule {
    /// The subtree this rule refuses, in [`spelling`] — the one form every
    /// comparison in this module is made in, never a `Path`, because
    /// `Path::starts_with` compares a `\\?\C:` prefix and a `C:` prefix as
    /// different things while they name one directory.
    prefix: Vec<String>,
    /// `prefix` followed by `**`, so a rule of §4's set reads the same way to
    /// [`Profile::rules`] as one the document wrote. It renders the prefix
    /// test exactly: `**` matches the empty tail, so the subtree's own root
    /// is covered.
    glob: Vec<String>,
    /// The project root, when this rule's subtree contains it, and nothing
    /// otherwise. It is the only exemption there is: a project checked out
    /// under `~/.config` must be able to read itself, and nothing else in
    /// `~/.config`. Dropping the rule instead handed the whole directory to
    /// any pattern that named it.
    ///
    /// Kept as a path because [`Rule::exempt_subtree`] hands it to a platform
    /// applier; `except_spelling` is the same subtree in the comparison form.
    except: Option<PathBuf>,
    except_spelling: Option<Vec<String>>,
    /// `true` for `.claude/**`, which is never writable but stays readable —
    /// `settings.json` is read before the sandbox is entered (§1.5).
    write_only: bool,
    rule: String,
}

/// The compiled profile.
///
/// Every field is private and every method takes a shared receiver, which is
/// the mechanism behind §1.1: there is no way to widen this after
/// [`Profile::compile`] returns.
#[derive(Debug, Clone)]
pub struct Profile {
    root: PathBuf,
    /// `root` in [`spelling`], compiled once. Every containment question this
    /// module asks about the project root is asked against this and never
    /// against `root` itself, for the reason [`NeverRule::prefix`] gives.
    root_spelling: Vec<String>,
    home: Option<PathBuf>,
    allow: Vec<PathRule>,
    deny: Vec<PathRule>,
    never: Vec<NeverRule>,
    command_allow: Vec<String>,
    command_deny: Vec<String>,
    mcp_allow: BTreeSet<String>,
    mcp_deny: BTreeSet<String>,
    diagnostics: Vec<String>,
}

/// The five `$HOME` directories §4.3 names, refusable by no pattern at all.
const NEVER_GRANTABLE_HOME: [&str; 5] = [".claude", ".codex", ".ssh", ".aws", ".config"];

/// Command names that re-enter the sandbox launcher or attach a debugger,
/// §4.6. Matched on every word of a command line, stripped of its directory
/// part, so `sh -c "sandbox-exec …"` is the same refusal as `sandbox-exec`.
const NEVER_GRANTABLE_COMMANDS: [&str; 12] = [
    "sandbox-exec",
    "bwrap",
    "bubblewrap",
    "lldb",
    "gdb",
    "strace",
    "ltrace",
    "dtrace",
    "dtruss",
    "windbg",
    "x64dbg",
    "vsjitdebugger.exe",
];

impl Profile {
    /// Compiles the profile a project's loaded configuration implies.
    ///
    /// This is the propagation path in one line: `project::load` fills
    /// `ProjectConfig::settings` with the document's exact bytes, and nothing
    /// between there and here repairs them.
    pub fn from_project(config: &ProjectConfig) -> Self {
        Self::compile(&config.root, config.settings.as_deref())
    }

    /// Compiles the profile for `root` from `settings`, the verbatim text of
    /// `.claude/settings.json`.
    ///
    /// A document that cannot be understood grants nothing rather than
    /// everything: it is untrusted input from a directory invariant 3 makes
    /// writable, so a parse failure, an unknown pattern kind and an unknown
    /// `permissions` key each add a diagnostic and no rule.
    pub fn compile(root: impl AsRef<Path>, settings: Option<&str>) -> Self {
        let root = resolve(root.as_ref(), None, None);
        let home = home_dir().map(|home| resolve(&home, None, None));
        let mut profile = Self {
            never: never_rules(&root, home.as_deref()),
            root_spelling: spelling(&root),
            root,
            home,
            allow: Vec::new(),
            deny: Vec::new(),
            command_allow: Vec::new(),
            command_deny: Vec::new(),
            mcp_allow: BTreeSet::new(),
            mcp_deny: BTreeSet::new(),
            diagnostics: Vec::new(),
        };
        let Some(text) = settings else {
            return profile;
        };
        let Ok(document) = serde_json::from_str::<serde_json::Value>(text) else {
            profile
                .diagnostics
                .push("`.claude/settings.json` is not valid JSON; it grants nothing".to_string());
            return profile;
        };
        let Some(permissions) = document.get("permissions") else {
            return profile;
        };
        let Some(permissions) = permissions.as_object() else {
            profile
                .diagnostics
                .push("`permissions` is not an object; it grants nothing".to_string());
            return profile;
        };
        for (key, value) in permissions {
            match key.as_str() {
                "allow" | "deny" => {}
                "ask" => {
                    profile.diagnostics.push(
                        "`permissions.ask` grants nothing: a refusal is a value that is returned, never a question put to a user"
                            .to_string(),
                    );
                    continue;
                }
                other => {
                    profile.diagnostics.push(format!(
                        "`permissions.{other}` is not a pattern list and grants nothing"
                    ));
                    continue;
                }
            }
            let denying = key == "deny";
            let Some(entries) = value.as_array() else {
                profile.diagnostics.push(format!(
                    "`permissions.{key}` is not an array; it grants nothing"
                ));
                continue;
            };
            for entry in entries {
                let Some(pattern) = entry.as_str() else {
                    profile.diagnostics.push(format!(
                        "a non-string entry in `permissions.{key}` grants nothing"
                    ));
                    continue;
                };
                register(&mut profile, pattern, denying);
            }
        }
        profile
    }
}

/// What a rule does to the paths it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// From `permissions.allow`.
    Allow,
    /// From `permissions.deny`, which beats every `allow` (§1.2).
    Deny,
    /// §4's set, which no pattern produced and none can undo.
    Never,
}

/// One compiled rule, as an applier may read it.
///
/// Every field is private and every accessor borrows, which is what makes
/// [`Profile::rules`] an enumeration rather than a second way in: there is no
/// constructor for this type outside the module, so nothing a caller holds
/// can become a grant.
#[derive(Debug, Clone, Copy)]
pub struct Rule<'a> {
    effect: Effect,
    written: &'a str,
    glob: &'a [String],
    read: bool,
    write: bool,
    except: Option<&'a Path>,
}

impl<'a> Rule<'a> {
    /// Allow, deny, or never-grantable.
    pub fn effect(&self) -> Effect {
        self.effect
    }

    /// The pattern as the document wrote it, or — for [`Effect::Never`] — the
    /// sentence a refusal quotes, since no document wrote those.
    pub fn written(&self) -> &'a str {
        self.written
    }

    /// The resolved path components, `*`, `?` and `**` intact. A component is
    /// matched the way [`Profile::check`] matches it: `*` and `?` do not
    /// cross a separator, and `**` spans any number of components.
    pub fn glob(&self) -> &'a [String] {
        self.glob
    }

    /// Whether the rule bears on reading, and on writing. Both are what
    /// [`Profile::check`] does with the rule rather than what its verb said,
    /// so an applier that renders these is as tight as the profile is.
    pub fn read(&self) -> bool {
        self.read
    }

    pub fn write(&self) -> bool {
        self.write
    }

    /// The one subtree this rule does not apply to: the project root, when a
    /// never-grantable directory contains it. `None` everywhere else.
    pub fn exempt_subtree(&self) -> Option<&'a Path> {
        self.except
    }
}

/// Maps one written pattern to its kind and records it, per §2's table.
///
/// A free function rather than a method, and deliberately: [`Profile`] has
/// no method at all that takes a mutable receiver, so the only place a rule
/// can be added is inside [`Profile::compile`], before the value exists for
/// anyone else to hold.
fn register(profile: &mut Profile, pattern: &str, denying: bool) {
    let (name, argument) = split_pattern(pattern);
    let (read, write) = match name {
        "Read" => (true, false),
        "Write" => (false, true),
        "Edit" => (true, true),
        "Bash" => {
            // Argv admission, and nothing in the filesystem profile. A
            // bare `Bash` admits every command line; the profile is
            // unchanged either way.
            let admitted = argument.unwrap_or("*").to_string();
            if denying {
                profile.command_deny.push(admitted);
            } else {
                if profile.command_allow.is_empty() {
                    // Said once, where a person can read it, rather than
                    // implied: what admits a command line here is a word scan
                    // and a segment match, and neither is a shell. A
                    // diagnostic is the mechanism this module uses everywhere
                    // else for "I did not act on that", and it is a compile-
                    // time value because a built profile can be told nothing
                    // afterwards (§1.1).
                    profile.diagnostics.push(
                        "argv admission is a word scan over each part of a command line, not a shell: a line that assembles a name through a variable, a substitution or a script file is admitted here, and the OS layer is what refuses it (sandbox-grants.md §4.6)"
                            .to_string(),
                    );
                }
                profile.command_allow.push(admitted);
            }
            return;
        }
        "WebFetch" | "WebSearch" => {
            // Network is never granted (§4.1). It is not registered and
            // not carried as a disabled rule: a network-needing tool is
            // absent, not present and failing.
            return;
        }
        other if other.starts_with("mcp__") => {
            if denying {
                profile.mcp_deny.insert(other.to_string());
            } else {
                profile.mcp_allow.insert(other.to_string());
            }
            return;
        }
        other => {
            profile.diagnostics.push(format!(
                "`{other}` is not a pattern kind this profile understands; `{pattern}` grants nothing"
            ));
            return;
        }
    };
    let Some(argument) = argument.map(str::trim).filter(|arg| !arg.is_empty()) else {
        profile.diagnostics.push(format!(
            "`{pattern}` names no path; a bare `{name}` grants nothing"
        ));
        return;
    };
    let glob = resolve_pattern(&profile.root, profile.home.as_deref(), argument);
    if glob.is_empty() {
        profile.diagnostics.push(format!(
            "`{pattern}` resolves to no path at all; it grants nothing"
        ));
        return;
    }
    // A pattern that names no root is anchored at the project root, and the
    // anchor is the whole of what "project-relative" means: `Read(../**)`
    // resolves to the project's parent, which is not a project-relative
    // grant by any reading. Refused rather than narrowed, and diagnosed, so
    // it is not a silent one.
    if !is_rooted(&argument.replace('\\', "/")) && !glob.starts_with(&profile.root_spelling[..]) {
        profile.diagnostics.push(format!(
            "`{pattern}` is project-relative and resolves outside the project root; it grants nothing"
        ));
        return;
    }
    let rule = PathRule {
        written: pattern.to_string(),
        glob,
        read,
        write,
    };
    if denying {
        // A `deny` entry refuses the paths it matches outright, whichever
        // verb spells it: §1.2 and §4.5 state the refusal path-wide, and
        // refusing more than was asked is the safe direction for a
        // document that is itself untrusted input.
        profile.deny.push(rule);
    } else {
        profile.allow.push(rule);
    }
}

impl Profile {
    /// The project root, resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every rule this profile compiled, in the order [`Profile::check`]
    /// consults them: §4's never-grantable set, then `deny`, then `allow`.
    ///
    /// **A platform applier cannot hold §1.2 for a rule it cannot see.** A
    /// profile that could only be asked about one path at a time left the
    /// seatbelt profile, the Landlock ruleset and the Windows ACL to be built
    /// from the project root alone, so an in-root
    /// `deny: ["Read(<root>/secrets/**)"]` was refused in process and granted
    /// by the kernel — §1.2 holding in one layer and not in the other, and
    /// the kernel is the layer that is supposed to be the backstop when the
    /// in-process check is wrong.
    ///
    /// Read-only, and structurally rather than by promise: [`Rule`] borrows
    /// this profile, has no public field and no constructor outside this
    /// module, so it can be rendered and there is no expression that turns
    /// one back into a grant (§1.1).
    pub fn rules(&self) -> impl Iterator<Item = Rule<'_>> {
        let never = self.never.iter().map(|rule| Rule {
            effect: Effect::Never,
            written: rule.rule.as_str(),
            glob: rule.glob.as_slice(),
            // What [`Profile::check`] does with it, not what a verb said: a
            // never rule refuses both halves unless it is the write-only
            // `.claude/**`.
            read: !rule.write_only,
            write: true,
            except: rule.except.as_deref(),
        });
        let deny = self.deny.iter().map(|rule| Rule {
            effect: Effect::Deny,
            written: rule.written.as_str(),
            glob: rule.glob.as_slice(),
            // Also what [`Profile::check`] does: a `deny` refuses the paths
            // it matches whichever verb spelled it, so an applier that
            // denied only the written half would be looser than the profile.
            read: true,
            write: true,
            except: None,
        });
        let allow = self.allow.iter().map(|rule| Rule {
            effect: Effect::Allow,
            written: rule.written.as_str(),
            glob: rule.glob.as_slice(),
            read: rule.read,
            write: rule.write,
            except: None,
        });
        never.chain(deny).chain(allow)
    }

    /// What the document said that this profile did not act on. Empty for a
    /// document every entry of which mapped to a rule.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// How many filesystem rules the document produced. A `Bash` or
    /// `WebFetch` pattern leaves this untouched, which is what makes §2's
    /// separation observable rather than asserted.
    pub fn rule_count(&self) -> usize {
        self.allow.len() + self.deny.len()
    }

    /// How many command-line patterns were admitted.
    pub fn command_pattern_count(&self) -> usize {
        self.command_allow.len()
    }

    /// How many MCP tool patterns were admitted. A pattern may glob, so this
    /// counts patterns rather than tools.
    pub fn mcp_tool_count(&self) -> usize {
        self.mcp_allow.len()
    }

    /// Whether this profile grants any network reach. Always `false`: no
    /// `permissions` pattern names a host, a port or a protocol, so a network
    /// grant would have to be invented, and an invented capability is the one
    /// thing an allow-list must never produce (§4.1).
    pub fn grants_network(&self) -> bool {
        false
    }

    /// Whether an MCP tool is registered. A tool matched by `deny` is not,
    /// and a network-needing tool never is.
    ///
    /// A tool name is matched by the same [`match_segment`] every path
    /// component is matched by, and with the same case decision: `deny`
    /// folds, `allow` does not. Exact-string equality here would have made
    /// `deny: ["mcp__git__*"]` deny nothing at all while every path pattern
    /// beside it globbed — a grant nobody asked for and no diagnostic.
    pub fn admits_mcp_tool(&self, name: &str) -> bool {
        if name.eq_ignore_ascii_case("webfetch") || name.eq_ignore_ascii_case("websearch") {
            return false;
        }
        if self
            .mcp_deny
            .iter()
            .any(|pattern| match_segment(pattern, name, true))
        {
            return false;
        }
        self.mcp_allow
            .iter()
            .any(|pattern| match_segment(pattern, name, false))
    }

    /// The first question of §2: may this command line be attempted at all?
    ///
    /// Answering `Ok` grants no file access whatsoever — the process it
    /// spawns gets exactly the grants the `Read`/`Write`/`Edit` patterns
    /// produced, which [`Profile::check`] is what answers.
    pub fn admits_command(&self, command_line: &str) -> Result<(), PermissionDenied> {
        let denied = |rule: String| -> Result<(), PermissionDenied> {
            Err(PermissionDenied {
                tool: "Bash".to_string(),
                path: command_line.to_string(),
                rule,
            })
        };
        if let Some(name) = escaping_command(command_line) {
            return denied(format!(
                "`{name}` re-enters the sandbox launcher or attaches a debugger and is never grantable by any pattern (sandbox-grants.md §4.6)"
            ));
        }
        // §1.2 is a rule about both of §2's questions, not only about paths.
        // A `deny` matched against the whole line lets an `allow` win by
        // concatenation — `cargo test -q; curl … | sh` is not `cargo test` —
        // so every part a shell would run as a command of its own is asked
        // separately, and one refused part refuses the line.
        let segments = command_segments(command_line);
        if segments.is_empty() {
            return denied(
                "no `Bash` pattern in permissions.allow admits this command line".to_string(),
            );
        }
        for segment in &segments {
            // A leading redirect is not the command: `2>&1 cargo test` is
            // matched on `cargo test`, never on the operand that happens to
            // come first. `segment` itself is still what a refusal quotes.
            let command_word = skip_leading_redirects(segment);
            for pattern in &self.command_deny {
                if match_segment(pattern, command_word, true) {
                    return denied(format!(
                        "`Bash({pattern})` in permissions.deny matches `{segment}`"
                    ));
                }
            }
            if !self
                .command_allow
                .iter()
                .any(|pattern| match_segment(pattern, command_word, false))
            {
                return denied(format!(
                    "no `Bash` pattern in permissions.allow admits `{segment}`"
                ));
            }
        }
        Ok(())
    }

    /// The second question of §2: may `tool` touch `path` for `access`, and
    /// **which path was that**?
    ///
    /// The returned `PathBuf` is the resolved path the decision was made on,
    /// and a caller must open that rather than the string it passed in. The
    /// two differ whenever the argument was spelled with a `~`, a relative
    /// prefix, a `.`, a `..` or a symlinked component, so a caller that
    /// re-opened its own argument would be opening a file this profile never
    /// examined.
    ///
    /// Decided in the only order that keeps §1.2 true — never-grantable
    /// first, then `deny`, then `allow` — so no `allow`, however exact, can
    /// reach past either. `path` is resolved before matching: `~` expands, a
    /// relative path resolves against the project root, and symlinks are
    /// resolved component by component, because two spellings of one path are
    /// how a containment check comes to disagree with itself.
    pub fn check(
        &self,
        tool: &str,
        access: Access,
        path: &Path,
    ) -> Result<PathBuf, PermissionDenied> {
        let resolved = resolve(path, Some(&self.root), self.home.as_deref());
        let shown = shown(&resolved);
        let candidate = spelling(&resolved);
        // The postcondition this function's own documentation states, held
        // where it is produced rather than where it is read: a granted path
        // is the absolute path the decision was made on, so opening it lands
        // on the file this profile examined. A root that is not absolute is
        // a literal-strings fixture — a Windows spelling on a Unix host —
        // and has no absolute form to demand (see `resolve`).
        let grant = |resolved: PathBuf| -> Result<PathBuf, PermissionDenied> {
            debug_assert!(
                resolved.is_absolute() || !self.root.is_absolute(),
                "`check` granted the non-absolute `{}`; a caller opening it would open a file this profile never examined",
                resolved.display()
            );
            Ok(resolved)
        };
        let denied = |rule: String| -> Result<PathBuf, PermissionDenied> {
            Err(PermissionDenied {
                tool: tool.to_string(),
                path: shown.clone(),
                rule,
            })
        };
        for never in &self.never {
            if never.write_only && access != Access::Write {
                continue;
            }
            if !contains(&never.prefix, &candidate) {
                continue;
            }
            // The project's own subtree, and only it, is exempt — and only
            // from a rule whose subtree contains the root.
            if let Some(except) = &never.except_spelling
                && contains(except, &candidate)
            {
                continue;
            }
            return denied(never.rule.clone());
        }
        for rule in &self.deny {
            if covers(&rule.glob, &candidate, true) {
                return denied(format!("`{}` in permissions.deny", rule.written));
            }
        }
        if contains(&self.root_spelling, &candidate) {
            return grant(resolved);
        }
        let granted = self.allow.iter().any(|rule| {
            let wanted = match access {
                Access::Read => rule.read,
                Access::Write => rule.write,
            };
            wanted && covers(&rule.glob, &candidate, false)
        });
        if granted {
            return grant(resolved);
        }
        denied(access.only_root_sentence().to_string())
    }
}

/// Builds §4's never-grantable set for one project root.
///
/// **No entry is ever dropped for where the project root sits.** A rule whose
/// subtree contains the root keeps the rule and exempts the root's own
/// subtree, so a project under `~/.config` can use `~/.config/myproj/**` and
/// still cannot touch `~/.config/gh/**`; a rule inside the root is kept as
/// written, so a project rooted at `$HOME` does not acquire `~/.ssh` by being
/// there. Skipping either shape — which is what this did — reached §4.3 and
/// §4.4 from an empty `permissions` object. `.claude/**` inside the root is
/// the one deliberate exception, and it is write-only.
///
/// Order is the message, not the answer: every rule below refuses, and the
/// specific entries are pushed before the whole of `$HOME` so a refusal cites
/// the section a person would look up.
fn never_rules(root: &Path, home: Option<&Path>) -> Vec<NeverRule> {
    let root_spelling = spelling(root);
    let dot_claude = spelling(&root.join(".claude"));
    let mut rules = vec![NeverRule {
        glob: subtree_glob(&dot_claude),
        prefix: dot_claude,
        except: None,
        except_spelling: None,
        write_only: true,
        rule: "`.claude/**` is never writable: a program that could edit it could widen the profile it was derived from (sandbox-grants.md §1.5)".to_string(),
    }];
    // `Some(root)`, and it is what makes §4.2 hold on Windows: `/etc/sudoers`
    // has a root and no drive there, so a candidate spelled that way acquires
    // the project's drive from `Path::join` and becomes `C:/etc/sudoers`. A
    // prefix resolved without the same root stayed `/etc/sudoers`, matched no
    // candidate at all, and `Write(/**)` reached the file. On macOS and Linux
    // every prefix here is already absolute, so the argument is never used.
    let mut push = |prefix: PathBuf, write_only: bool, rule: String| {
        let prefix = resolve(&prefix, Some(root), None);
        let prefix = spelling(&prefix);
        let except = contains(&prefix, &root_spelling).then(|| root.to_path_buf());
        let mut glob = prefix.clone();
        glob.push("**".to_string());
        rules.push(NeverRule {
            except_spelling: except.as_ref().map(|_| root_spelling.clone()),
            prefix,
            glob,
            except,
            write_only,
            rule,
        });
    };
    let keyring = "the OS keyring or credential store is never grantable by any pattern (sandbox-grants.md §4.2)";
    if let Some(home) = home {
        for name in NEVER_GRANTABLE_HOME {
            push(
                home.join(name),
                false,
                format!("`~/{name}` is never grantable by any pattern (sandbox-grants.md §4.3)"),
            );
        }
        for path in [
            home.join("Library").join("Keychains"),
            home.join(".local").join("share").join("keyrings"),
            home.join(".gnupg"),
        ] {
            push(path, false, keyring.to_string());
        }
    }
    push(
        PathBuf::from("/Library/Keychains"),
        false,
        keyring.to_string(),
    );
    for state in glasshouse_state_dirs(home) {
        push(
            state,
            false,
            "Glasshouse's own state and data directories, and every database in them, are never grantable by any pattern (sandbox-grants.md §4.4)".to_string(),
        );
    }
    for path in system_credential_paths() {
        push(path, true, keyring.to_string());
    }
    if let Some(home) = home {
        // §4.3 as it is titled: **`$HOME` outside the project**, whatever
        // pattern names it. The five names in that sentence are the ones that
        // matter, not the whole of what it refuses — a sandbox that lets a
        // tool rewrite `~/.gitconfig` or `~/.zsh_history` is not holding a
        // boundary. Last, so the named entries above keep their own sections.
        push(
            home.to_path_buf(),
            false,
            "`$HOME` outside the project is never grantable by any pattern (sandbox-grants.md §4.3)"
                .to_string(),
        );
    }
    rules
}

/// A resolved subtree as a glob: its spelling, then `**`.
fn subtree_glob(prefix: &[String]) -> Vec<String> {
    let mut glob = prefix.to_vec();
    glob.push("**".to_string());
    glob
}

/// The machine's own credential and identity store — §4.2's system half,
/// which is what stops `Write(/**)` reaching `/etc/sudoers`.
///
/// Named files and directories rather than the whole of `/etc`, because
/// `/etc/hosts` is an ordinary readable file and §3's own seatbelt shape
/// reads `/etc/passwd`; and write-only at the call site above for the same
/// reason. Computed without a `#[cfg]`, so a profile compiled on one host
/// still refuses another host's spelling.
fn system_credential_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = [
        "/etc/sudoers",
        "/etc/sudoers.d",
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/passwd",
        "/etc/master.passwd",
        "/etc/group",
        "/etc/pam.d",
        "/etc/ssh",
        "/etc/security",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    for key in ["SystemRoot", "windir"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            paths.push(PathBuf::from(value).join("System32").join("config"));
        }
    }
    paths
}

/// Every shape Glasshouse's own state and data directories take, on every
/// platform, computed without a `#[cfg]` so a profile compiled on one host
/// still refuses the paths another host would use.
fn glasshouse_state_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in [
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(home) = home {
        roots.push(home.join(".local").join("share"));
        roots.push(home.join(".local").join("state"));
        roots.push(home.join(".cache"));
        roots.push(home.join("Library").join("Application Support"));
        roots.push(home.join("Library").join("Caches"));
        roots.push(home.join("AppData").join("Roaming"));
        roots.push(home.join("AppData").join("Local"));
    }
    let mut dirs: Vec<PathBuf> = roots
        .into_iter()
        .map(|root| root.join("glasshouse"))
        .collect();
    if let Some(home) = home {
        dirs.push(home.join(".glasshouse"));
    }
    for key in ["GLASSHOUSE_STATE_DIR", "GLASSHOUSE_DATA_DIR"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            dirs.push(PathBuf::from(value));
        }
    }
    dirs
}

/// **This scan cannot catch a name the shell assembles**: `sh -c 'S=sandbox-exec;
/// $S …'`, a `$(printf …)` that builds one, or a `sh ./run.sh` whose script
/// the project root makes writable — no word of any of those lines names an
/// escape, and no word list closes that, because the shell is a general
/// interpreter. §4.6 is held by the platform appliers (seatbelt's
/// `(deny process-exec* …)`, Landlock plus `no_new_privs`, the AppContainer);
/// this function is a cheap early refusal in front of them and nothing more.
///
/// What it does catch: the word of `command_line` naming one of §4.6's
/// escapes, if any. Every word is examined rather than the first, because
/// `sh -c "bwrap …"` and `sudo lldb` are the spellings a first-word check
/// walks straight past; a command line that merely mentions one of these
/// names is refused too, which is the direction a never-grantable set must
/// err in.
fn escaping_command(command_line: &str) -> Option<&'static str> {
    command_line.split_whitespace().find_map(|word| {
        let base = word
            .trim_matches(['"', '\'', '`', '(', ')', ';', '&', '|'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(word);
        NEVER_GRANTABLE_COMMANDS
            .iter()
            .copied()
            .find(|name| base.eq_ignore_ascii_case(name))
    })
}

/// `$HOME`, read from the environment rather than from a platform helper so
/// this stays one code path on every host.
fn home_dir() -> Option<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            return Some(PathBuf::from(value));
        }
    }
    None
}

/// Resolves a candidate path the way every comparison in this module needs
/// it: `~` expands, a relative path resolves against `root`, and the
/// components are resolved **in the order the kernel would follow them**, so
/// a `..` is applied to the directory a call would be standing in rather than
/// to the name as written.
///
/// That order is the invariant, and it holds because [`canonical_prefix`] is
/// applied to the accumulator *before* every `ParentDir` pop rather than to
/// the whole string afterwards. Popping textually first is how
/// `<root>/link/../.ssh/id_ed25519` came to be `<root>/.ssh/id_ed25519` here
/// — a path inside the project — while every real call landed in `$HOME`.
///
/// The tail that does not exist yet — a file about to be created, which can
/// therefore be no symlink — is appended to the resolved prefix, so a write
/// check decides on the same spelling a later read of that file would.
///
/// **[`Profile::check`] never returns a non-absolute path on a host whose
/// root is absolute**, and the anchoring condition is the whole of why. A
/// candidate is read in *the root's own spelling family*: a drive-rooted
/// `C:\x` is already rooted under a `\\?\C:\proj` or `C:/proj` root and
/// must not acquire the project's prefix a second time, while under
/// `/Users/…/proj` the same string is one relative filename that happens to
/// contain backslashes, and leaving it unanchored granted a **relative**
/// path — the file a caller then opened was not the file the decision was
/// made on.
fn resolve(path: &Path, root: Option<&Path>, home: Option<&Path>) -> PathBuf {
    let mut expanded = expand_tilde(path, home);
    if let Some(root) = root
        && !expanded.is_absolute()
        && !(windows_rooted(&expanded) && windows_rooted(root))
    {
        expanded = root.join(expanded);
    }
    let mut out = PathBuf::new();
    for component in expanded.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out = canonical_prefix(&out);
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    canonical_prefix(&out)
}

/// `path` with the longest prefix of it that exists replaced by its canonical
/// form, and the components that do not exist appended as written.
fn canonical_prefix(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut prefix = path.to_path_buf();
    while !prefix.exists() {
        let Some(name) = prefix.file_name().map(|name| name.to_os_string()) else {
            break;
        };
        tail.push(name);
        if !prefix.pop() {
            break;
        }
    }
    let mut resolved = std::fs::canonicalize(&prefix).unwrap_or(prefix);
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    resolved
}

fn expand_tilde(path: &Path, home: Option<&Path>) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return path.to_path_buf();
    }
    let Some(home) = home.map(PathBuf::from).or_else(home_dir) else {
        return path.to_path_buf();
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        home
    } else {
        home.join(rest)
    }
}

/// Every part of a command line a shell would run as a command of its own:
/// the sequence and pipeline operators (`;`, `&&`, `||`, `|`, a background
/// `&` and a newline), plus the contents of a command substitution (`$(…)`,
/// backticks), which is a command line in its own right — but **not** a `&`
/// that is part of a redirect operator (`2>&1`, `>&2`, `<&0`, `&>file`,
/// `&>>file`), because that `&` never starts a new command.
///
/// Deliberately not a shell parser: quoting is not tracked, so a literal `;`
/// inside quotes splits too. That asks about more parts than a shell would
/// run, which is the refusing direction, and it is why this can be a dozen
/// lines rather than a grammar.
fn command_segments(command_line: &str) -> Vec<String> {
    let chars: Vec<char> = command_line.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '$' if chars.get(index + 1) == Some(&'(') => {
                let (inner, next) = balanced(&chars, index + 2, Some('('), ')');
                out.extend(command_segments(&inner));
                index = next;
            }
            '`' => {
                let (inner, next) = balanced(&chars, index + 1, None, '`');
                out.extend(command_segments(&inner));
                index = next;
            }
            '&' if is_redirect_ampersand(&chars, index) => {
                current.push('&');
                index += 1;
            }
            ';' | '\n' | '&' | '|' => {
                flush_segment(&mut out, &mut current);
                index += 1;
            }
            other => {
                current.push(other);
                index += 1;
            }
        }
    }
    flush_segment(&mut out, &mut current);
    out
}

/// Whether the `&` at `index` is part of a redirect operator rather than a
/// background or `&&` operator: immediately after a `>` or `<` (`2>&1`,
/// `<&0`), or immediately before a `>` (`&>file`, `&>>file`). A leading
/// file-descriptor digit needs no separate check here — it was already an
/// ordinary character pushed onto the current segment before this `&` was
/// reached.
fn is_redirect_ampersand(chars: &[char], index: usize) -> bool {
    let prev = index.checked_sub(1).and_then(|i| chars.get(i));
    let next = chars.get(index + 1);
    matches!(prev, Some('>') | Some('<')) || matches!(next, Some('>'))
}

/// Whether `word` is entirely one redirect operator with its operand
/// attached — `N>file`, `N>&M`, `<file`, `>file`, `>>file`, `&>file`,
/// `&>>file` — the same forms [`command_segments`] no longer splits on.
fn is_redirect_word(word: &str) -> bool {
    if let Some(rest) = word.strip_prefix("&>>").or_else(|| word.strip_prefix("&>")) {
        return !rest.is_empty();
    }
    let digits_end = word
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(word.len());
    let rest = &word[digits_end..];
    let operand = rest
        .strip_prefix(">>")
        .or_else(|| rest.strip_prefix('>'))
        .or_else(|| rest.strip_prefix('<'));
    matches!(operand, Some(operand) if !operand.is_empty())
}

/// `segment` with every leading redirect word removed, so matching begins at
/// the command: `2>&1 cargo test` becomes `cargo test`, while `1 cargo test`
/// — a literal word, not an operator — is returned unchanged.
fn skip_leading_redirects(segment: &str) -> &str {
    let mut rest = segment.trim_start();
    loop {
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..word_end];
        if word.is_empty() || !is_redirect_word(word) {
            return rest;
        }
        rest = rest[word_end..].trim_start();
    }
}

/// The text up to the `close` that balances the one already consumed, and the
/// index just past it. `open` is `None` where the delimiter cannot nest.
fn balanced(chars: &[char], start: usize, open: Option<char>, close: char) -> (String, usize) {
    let mut depth = 1usize;
    let mut inner = String::new();
    let mut index = start;
    while index < chars.len() {
        let c = chars[index];
        if c == close {
            depth -= 1;
            if depth == 0 {
                return (inner, index + 1);
            }
        } else if Some(c) == open {
            depth += 1;
        }
        inner.push(c);
        index += 1;
    }
    (inner, index)
}

fn flush_segment(out: &mut Vec<String>, current: &mut String) {
    let segment = current.trim().to_string();
    current.clear();
    if !segment.is_empty() {
        out.push(segment);
    }
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// One spelling of one path, and the only form this module ever compares two
/// paths in.
///
/// **Two spellings of one path are how a containment check comes to disagree
/// with itself** (§2), and on Windows one file has three: `fs::canonicalize`
/// returns the *verbatim* `\\?\C:\…`, an environment variable or a settings
/// pattern returns the ordinary `C:\…`, and either may carry an 8.3 short
/// name such as `RUNNER~1`. This function decides the first two — `\` folded
/// to `/`, a verbatim prefix reduced to the ordinary spelling it stands for,
/// the drive letter upper-cased — and [`canonical_prefix`] decides the third,
/// because only the filesystem that issued a short name can say what it is
/// short for.
///
/// The fold is unconditional and always was: `\` is a separator here on every
/// host, which is what lets one matcher serve all three. The *reduction* is
/// not, and that condition is a containment rule rather than tidiness — see
/// [`reduced_verbatim`].
fn spelling(path: &Path) -> Vec<String> {
    let folded = display(path).replace('\\', "/");
    let text = reduced_verbatim(&folded).unwrap_or(folded);
    let mut parts: Vec<String> = text
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    // The **drive letter**, and not the component it begins. `C:foo` and
    // `C:FOO` are two files on a case-sensitive filesystem, and folding the
    // whole component made an `allow` match case-insensitively there —
    // against `match_segment`'s rule that an `allow` never folds, whose
    // reason is that folding lets one spelling reach a path its author never
    // wrote. Only the first character is touched; `is_drive_prefixed`
    // guarantees it is ASCII, so byte 1 is a character boundary.
    if let Some(first) = parts.first_mut()
        && is_drive_prefixed(first)
    {
        first[..1].make_ascii_uppercase();
    }
    parts
}

/// `folded` without its Windows verbatim prefix, or `None` when it carries
/// none — where "carries one" means `//?/` followed by something only Windows
/// produces.
///
/// That condition is the isolation half of the reduction. `//?/` is an
/// unusual but perfectly legal absolute path on Unix, so an unconditional
/// strip would reduce `//?/proj/a.rs` to the *relative* `proj/a.rs`, which
/// [`resolve`] then anchors inside the project root. Requiring a drive letter
/// or the `UNC/` marker is what keeps the Windows repair from widening
/// containment on every other platform — the same test, for the same reason,
/// as `crates/glasshouse/src/commands/context_firewall.rs`.
///
/// **On Unix this reduction is reachable only from a backslash-spelled
/// candidate.** `Path::components` collapses `//` before [`spelling`] sees
/// it, so a `/`-spelled `//?/…` argument never carries the prefix this
/// function tests for by the time it is asked — which means
/// `a_verbatim_and_a_plain_spelling_of_one_path_decide_identically`
/// exercises the `\\?\` arm on this host and never the `//?/` one. It is
/// not cross-platform cover for that arm.
fn reduced_verbatim(folded: &str) -> Option<String> {
    let rest = folded.strip_prefix("//?/")?;
    if is_drive_prefixed(rest) {
        return Some(rest.to_string());
    }
    // `\\?\UNC\srv\share` is the verbatim way of writing `\\srv\share`: the
    // marker stands in for the second leading separator.
    let marker = rest.get(..4)?;
    marker
        .eq_ignore_ascii_case("unc/")
        .then(|| format!("//{}", &rest[4..]))
}

/// Whether `text` begins with a drive letter and a colon.
fn is_drive_prefixed(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

/// Whether `path` is already rooted in a spelling only Windows produces: a
/// drive letter, a verbatim prefix, or a UNC share.
///
/// Asked on every host, and deliberately. [`is_rooted`] already asks exactly
/// this of a written *pattern* without a `#[cfg]`, and a candidate path that
/// got a different answer is the disagreement this module exists to prevent —
/// a never-rule computed for a Windows spelling could never meet a candidate
/// that had been anchored inside the project instead. On macOS and Linux the
/// question is only ever put to a path that is not already absolute, so
/// nothing beginning `/` reaches it and the only spelling that can match is a
/// literal drive letter, which no path in a Unix project is.
fn windows_rooted(path: &Path) -> bool {
    let text = display(path).replace('\\', "/");
    text.starts_with("//") || is_drive_prefixed(&text)
}

/// `path` as one string, for the sentence a person reads.
///
/// The verbatim prefix is reduced here too, so a refusal quotes the spelling
/// the settings file would use rather than the one the kernel handed back.
/// Nothing else is touched: a path carrying no verbatim prefix — which is
/// every path on macOS and Linux — is returned exactly as it was, separators
/// included.
fn shown(path: &Path) -> String {
    let text = display(path);
    reduced_verbatim(&text.replace('\\', "/")).unwrap_or(text)
}

/// Whether `candidate` is `prefix` or lies beneath it, in [`spelling`].
fn contains(prefix: &[String], candidate: &[String]) -> bool {
    prefix.len() <= candidate.len() && prefix.iter().zip(candidate).all(|(a, b)| a == b)
}

/// Whether a written pattern names an absolute path, a `~`-rooted one, or a
/// Windows drive — the three that are not resolved against the project root.
fn is_rooted(pattern: &str) -> bool {
    pattern.starts_with('/')
        || pattern == "~"
        || pattern.starts_with("~/")
        || pattern
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':' && pattern.as_bytes()[0].is_ascii_alphabetic())
}

/// Resolves a written pattern's literal prefix and keeps its glob tail.
///
/// The prefix is resolved so a pattern and a candidate spelled differently —
/// `/tmp/…` against `/private/tmp/…` — cannot disagree; the tail is left
/// alone because a glob names no single path to resolve. A pattern that
/// names no root is anchored at the project root before either step; a
/// pattern whose resolved form then leaves the root is refused by
/// [`register`] rather than compiled, which is what makes it true that a
/// project-relative glob cannot match its way out of the project.
fn resolve_pattern(root: &Path, home: Option<&Path>, pattern: &str) -> Vec<String> {
    // Both halves are reduced before they are spliced, and the pattern's own
    // half matters as much as the root's: `?` is a glob metacharacter here, so
    // a verbatim `//?/C:/…` left in either one splits at the `?` and anchors
    // the pattern under a doubled drive prefix that matches no path at all.
    // That is what made every project-relative pattern — `Read(**)`,
    // `Read(src/**)` — register nothing on Windows.
    let normalized = ordinary(&pattern.replace('\\', "/"));
    let anchored = if is_rooted(&normalized) {
        normalized
    } else {
        format!(
            "{}/{normalized}",
            ordinary(&display(root).replace('\\', "/"))
        )
    };
    let mut literal = Vec::new();
    let mut rest = Vec::new();
    for part in anchored.split('/') {
        if rest.is_empty() && !part.contains(['*', '?']) {
            literal.push(part.to_string());
        } else {
            rest.push(part.to_string());
        }
    }
    let literal_path = literal.join("/");
    let mut out = if literal_path.is_empty() {
        Vec::new()
    } else {
        spelling(&resolve(Path::new(&literal_path), Some(root), home))
    };
    out.extend(rest.into_iter().filter(|part| !part.is_empty()));
    out
}

/// `folded` with a Windows verbatim prefix reduced, and unchanged otherwise.
fn ordinary(folded: &str) -> String {
    reduced_verbatim(folded).unwrap_or_else(|| folded.to_string())
}

/// Splits `Name(argument)` into its parts; a bare `Name` has no argument.
fn split_pattern(pattern: &str) -> (&str, Option<&str>) {
    let pattern = pattern.trim();
    let Some(open) = pattern.find('(') else {
        return (pattern, None);
    };
    if !pattern.ends_with(')') {
        return (pattern, None);
    }
    (
        &pattern[..open],
        Some(&pattern[open + 1..pattern.len() - 1]),
    )
}

/// Whether `glob` matches `candidate` or names one of its ancestors.
///
/// Naming an ancestor is a match because a pattern that names a directory
/// covers its subtree — the `(subpath …)` term §3's seatbelt shape uses, and
/// the "realpath closure of the glob" §2's table names.
fn covers(glob: &[String], candidate: &[String], fold: bool) -> bool {
    (0..=candidate.len()).any(|end| match_components(glob, &candidate[..end], fold))
}

fn match_components(glob: &[String], candidate: &[String], fold: bool) -> bool {
    let Some((head, tail)) = glob.split_first() else {
        return candidate.is_empty();
    };
    if head == "**" {
        return (0..=candidate.len()).any(|skip| match_components(tail, &candidate[skip..], fold));
    }
    let Some((first, rest)) = candidate.split_first() else {
        return false;
    };
    match_segment(head, first, fold) && match_components(tail, rest, fold)
}

/// Matches one component, where `*` and `?` do not cross a separator.
///
/// `fold` is the case-sensitivity decision, and it is deliberate rather than
/// the host's: **a `deny` pattern matches case-insensitively on every
/// platform and an `allow` pattern matches case-sensitively on every
/// platform.** A case-insensitive filesystem would otherwise let
/// `SECRET.ENV` walk past a `deny` written as `secret.env`, and folding
/// `allow` instead would let the same trick reach a path its author never
/// spelled. Both errors are made in the refusing direction, and the answer is
/// the same on macOS, Linux and Windows rather than three answers.
fn match_segment(pattern: &str, text: &str, fold: bool) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    match_chars(&pattern, &text, fold)
}

fn match_chars(pattern: &[char], text: &[char], fold: bool) -> bool {
    let Some((head, tail)) = pattern.split_first() else {
        return text.is_empty();
    };
    match head {
        '*' => (0..=text.len()).any(|skip| match_chars(tail, &text[skip..], fold)),
        '?' => !text.is_empty() && match_chars(tail, &text[1..], fold),
        expected => match text.split_first() {
            Some((actual, rest)) if same(*expected, *actual, fold) => match_chars(tail, rest, fold),
            _ => false,
        },
    }
}

fn same(expected: char, actual: char, fold: bool) -> bool {
    if expected == actual {
        return true;
    }
    fold && expected.to_lowercase().eq(actual.to_lowercase())
}
