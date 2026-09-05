//! Resolves a slash command name to what it *is*, never to its effect: this
//! module runs nothing, spawns nothing, and reads no content it is handed
//! back as anything but a string. It only decides, for a given
//! `contract::ProjectConfig` and name, which of a built-in, a project
//! command, or a skill answers, and whether the built-in's subsystem exists
//! yet.

use crate::contract::ProjectConfig;

/// The seven names map line 2450 requires pane to offer regardless of the
/// project, in the order they are listed there.
pub const BUILT_INS: [BuiltIn; 7] = [
    BuiltIn::Model,
    BuiltIn::Entitlements,
    BuiltIn::Handles,
    BuiltIn::Supervisor,
    BuiltIn::Rollback,
    BuiltIn::Budget,
    BuiltIn::Memory,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltIn {
    Model,
    Entitlements,
    Handles,
    Supervisor,
    Rollback,
    Budget,
    Memory,
}

impl BuiltIn {
    /// The command's name, without the leading `/` -- the same shape as a
    /// project command's key, so the two compare directly.
    pub fn name(self) -> &'static str {
        match self {
            BuiltIn::Model => "model",
            BuiltIn::Entitlements => "entitlements",
            BuiltIn::Handles => "handles",
            BuiltIn::Supervisor => "supervisor",
            BuiltIn::Rollback => "rollback",
            BuiltIn::Budget => "budget",
            BuiltIn::Memory => "memory",
        }
    }

    /// The sub-phase that has not landed yet, for a built-in that names a
    /// subsystem this round does not build. `None` means the built-in's
    /// subsystem already exists.
    pub fn pending_subphase(self) -> Option<&'static str> {
        match self {
            BuiltIn::Handles => Some("61E"),
            BuiltIn::Supervisor => Some("61F"),
            _ => None,
        }
    }
}

/// Where a resolved command's answer came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    BuiltIn(BuiltIn),
    ProjectCommand,
    ProjectSkill,
}

/// Whether invoking a resolved command would do anything yet.
///
/// This is a state on the resolved command rather than an error, because a
/// built-in naming a subsystem that has not landed (`/handles` is 61E,
/// `/supervisor` is 61F) is not a failure to resolve -- map line 2450 says
/// to offer it -- it is a command that exists and honestly reports it is not
/// wired to anything yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandStatus {
    Available,
    NotBuilt { subphase: &'static str },
}

/// A slash command by name, and what would happen if it were invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub name: String,
    pub source: CommandSource,
    pub status: CommandStatus,
}

/// Every command pane offers for `project`: the built-ins, then every
/// project command and skill not shadowed by one.
///
/// **A built-in's name is reserved and always wins a collision.** A
/// project's own commands and skills are the project's own untrusted text,
/// and `/rollback` or `/budget` naming safety-relevant behaviour must not be
/// silently replaced by a same-named file the project happens to ship. Where
/// a project command and a skill share a name that no built-in claims, the
/// command wins: `.claude/commands/<name>.md` is written to be a command,
/// where a skill of the same name is a directory pane merely offers by
/// name, so the more specific source takes it. Both rules are one
/// precedence order: built-in, then project command, then skill.
pub fn all(project: &ProjectConfig) -> Vec<ResolvedCommand> {
    let mut out: Vec<ResolvedCommand> = BUILT_INS
        .iter()
        .map(|&builtin| resolved_builtin(builtin))
        .collect();

    for name in project.commands.keys() {
        if BUILT_INS.iter().any(|b| b.name() == name) {
            continue;
        }
        out.push(ResolvedCommand {
            name: name.clone(),
            source: CommandSource::ProjectCommand,
            status: CommandStatus::Available,
        });
    }

    for name in project.skills.keys() {
        if BUILT_INS.iter().any(|b| b.name() == name) {
            continue;
        }
        if project.commands.contains_key(name) {
            continue;
        }
        out.push(ResolvedCommand {
            name: name.clone(),
            source: CommandSource::ProjectSkill,
            status: CommandStatus::Available,
        });
    }

    out
}

/// Resolves one name against `project`, following the same precedence as
/// [`all`]: built-in, then project command, then skill.
pub fn resolve(project: &ProjectConfig, name: &str) -> Option<ResolvedCommand> {
    if let Some(builtin) = BUILT_INS.iter().find(|b| b.name() == name) {
        return Some(resolved_builtin(*builtin));
    }
    if project.commands.contains_key(name) {
        return Some(ResolvedCommand {
            name: name.to_string(),
            source: CommandSource::ProjectCommand,
            status: CommandStatus::Available,
        });
    }
    if project.skills.contains_key(name) {
        return Some(ResolvedCommand {
            name: name.to_string(),
            source: CommandSource::ProjectSkill,
            status: CommandStatus::Available,
        });
    }
    None
}

fn resolved_builtin(builtin: BuiltIn) -> ResolvedCommand {
    let status = match builtin.pending_subphase() {
        Some(subphase) => CommandStatus::NotBuilt { subphase },
        None => CommandStatus::Available,
    };
    ResolvedCommand {
        name: builtin.name().to_string(),
        source: CommandSource::BuiltIn(builtin),
        status,
    }
}
