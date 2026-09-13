//! The effective capability and environment manifest the parent model reads
//! before acting -- `smarter-cheaper-roadmap.md`, *Capability/environment
//! manifest*.
//!
//! The invariant: **every line here is a fact the compiled profile or the
//! host already decided, rendered once.** Nothing in this module grants,
//! probes for permission or widens anything; it reports what
//! `sandbox::profile::Profile` will answer so a model does not spend a turn
//! discovering a refusal that was decidable at session start. The Terminal-
//! Bench pilot paid six of thirteen failed cells for exactly that
//! (`gdb` refused three times, `/build` refused three times).

use std::fmt::Write as _;

/// How command lines are admitted, as the manifest states it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CommandPolicy {
    /// Every command line is admitted (`--yolo` or a bare `Bash` grant).
    All,
    /// Only command lines matching one of these `Bash(...)` patterns.
    Patterns(Vec<String>),
    /// No command may run at all.
    #[default]
    None,
}

/// One executable the host looked for on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    pub name: String,
    /// The resolved path, or `None` when the executable is absent.
    pub path: Option<String>,
}

/// The manifest, as data. `collect` fills it from a compiled profile; `render`
/// is the one text form the prompt carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// The project root every relative path resolves against.
    pub root: String,
    /// Directories readable in full, in the order the profile grants them.
    pub readable_roots: Vec<String>,
    /// Directories writable in full.
    pub writable_roots: Vec<String>,
    /// Paths inside a writable root that are never writable (`.pane/**`,
    /// `.claude/**`), so a scratch file goes elsewhere.
    pub reserved_paths: Vec<String>,
    /// Configured `deny` patterns, spelled as the profile holds them.
    pub denied_patterns: Vec<String>,
    /// How command lines are admitted.
    pub commands: CommandPolicy,
    /// Commands refused by name whatever the grant says.
    pub never_grantable_commands: Vec<String>,
    /// Executables the host looked for, present or absent.
    pub executables: Vec<Executable>,
    /// Whether the sandbox grants shell network access.
    pub network: bool,
    /// True when the session runs under the explicit outer-container bypass:
    /// reads span the container and a debugger is admissible, because the
    /// container is the process boundary rather than Pane's own sandbox.
    pub container_mode: bool,
    /// Capabilities the model might reach for that this session cannot
    /// provide, each with the reason, one line each.
    pub unavailable: Vec<String>,
}

impl Manifest {
    /// The `## Environment` block, bounded and deterministic for one profile.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("## Environment\n\n");
        let _ = writeln!(out, "Project root: {}", self.root);
        let _ = writeln!(
            out,
            "Readable roots: {}",
            if self.readable_roots.is_empty() {
                "(none)".to_string()
            } else {
                self.readable_roots.join(", ")
            }
        );
        let _ = writeln!(
            out,
            "Writable roots: {}",
            if self.writable_roots.is_empty() {
                "(none)".to_string()
            } else {
                self.writable_roots.join(", ")
            }
        );
        if !self.reserved_paths.is_empty() {
            let _ = writeln!(
                out,
                "Reserved (never writable; put scratch files elsewhere under a writable root): {}",
                self.reserved_paths.join(", ")
            );
        }
        if !self.denied_patterns.is_empty() {
            let _ = writeln!(out, "Denied patterns: {}", self.denied_patterns.join(", "));
        }
        match &self.commands {
            CommandPolicy::All => {
                let _ = writeln!(out, "Commands: every command line is admitted");
            }
            CommandPolicy::Patterns(patterns) => {
                let _ = writeln!(
                    out,
                    "Commands: only lines matching {} pattern(s): {}",
                    patterns.len(),
                    patterns.join(", ")
                );
            }
            CommandPolicy::None => {
                let _ = writeln!(out, "Commands: none may run");
            }
        }
        if !self.never_grantable_commands.is_empty() {
            let _ = writeln!(
                out,
                "Never admitted by name: {}",
                self.never_grantable_commands.join(", ")
            );
        }
        let present: Vec<String> = self
            .executables
            .iter()
            .filter_map(|e| e.path.as_ref().map(|p| format!("{}={p}", e.name)))
            .collect();
        let absent: Vec<&str> = self
            .executables
            .iter()
            .filter(|e| e.path.is_none())
            .map(|e| e.name.as_str())
            .collect();
        if !present.is_empty() {
            let _ = writeln!(out, "Available executables: {}", present.join("; "));
        }
        if !absent.is_empty() {
            let _ = writeln!(out, "Absent executables: {}", absent.join(", "));
        }
        let _ = writeln!(
            out,
            "Network from shell: {}",
            if self.network { "yes" } else { "no" }
        );
        if self.container_mode {
            let _ = writeln!(
                out,
                "Container mode: the outer container is the process boundary; reads span it and a debugger may run."
            );
        }
        for line in &self.unavailable {
            let _ = writeln!(out, "Unavailable: {line}");
        }
        out.trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            root: "/app".into(),
            readable_roots: vec!["/app".into(), "/build".into()],
            writable_roots: vec!["/app".into()],
            reserved_paths: vec!["/app/.pane".into(), "/app/.claude".into()],
            denied_patterns: vec![],
            commands: CommandPolicy::All,
            never_grantable_commands: vec!["sandbox-exec".into(), "bwrap".into()],
            executables: vec![
                Executable {
                    name: "gdb".into(),
                    path: Some("/usr/bin/gdb".into()),
                },
                Executable {
                    name: "rg".into(),
                    path: None,
                },
            ],
            network: false,
            container_mode: true,
            unavailable: vec!["web.search: no search endpoint is configured".into()],
        }
    }

    #[test]
    fn the_manifest_names_roots_reserved_paths_and_absent_executables() {
        let rendered = sample().render();
        assert!(rendered.starts_with("## Environment"), "{rendered}");
        assert!(
            rendered.contains("Readable roots: /app, /build"),
            "{rendered}"
        );
        assert!(rendered.contains("Writable roots: /app"), "{rendered}");
        assert!(rendered.contains("Reserved (never writable"), "{rendered}");
        assert!(rendered.contains("/app/.pane"), "{rendered}");
        assert!(
            rendered.contains("Available executables: gdb=/usr/bin/gdb"),
            "{rendered}"
        );
        assert!(rendered.contains("Absent executables: rg"), "{rendered}");
        assert!(rendered.contains("Container mode"), "{rendered}");
        assert!(rendered.contains("Unavailable: web.search"), "{rendered}");
    }

    #[test]
    fn an_empty_manifest_still_says_what_it_does_not_have() {
        let rendered = Manifest {
            commands: CommandPolicy::None,
            ..Manifest::default()
        }
        .render();
        assert!(rendered.contains("Readable roots: (none)"), "{rendered}");
        assert!(rendered.contains("Commands: none may run"), "{rendered}");
        assert!(!rendered.contains("Container mode"), "{rendered}");
    }

    #[test]
    fn rendering_is_deterministic_for_one_manifest() {
        assert_eq!(sample().render(), sample().render());
    }
}
