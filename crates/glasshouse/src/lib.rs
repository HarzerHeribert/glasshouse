//! Glasshouse — a lean, project-scoped control plane for native coding-agent
//! harnesses.
//!
//! Glasshouse starts and manages real native harness sessions rather than
//! replacing them. One instance operates on exactly one project root, and every
//! piece of state it keeps is physically separated per project.

pub mod checkpoint;
pub mod cli;
pub mod config;
mod database;
pub mod events;
pub mod gateway;
pub mod harness;
pub mod integrations;
pub mod launch;
pub mod logging;
pub mod memory;
pub mod onboarding;
pub mod paths;
pub mod platform;
pub mod profile;
pub mod project;
pub mod provider;
pub mod pty;
pub mod routing;
pub mod secret;
pub mod session;
pub mod shell;
pub mod shim;
pub mod shutdown;
pub mod tui;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use cli::{Cli, Command, MemoryCommand};
pub use paths::RuntimePaths;
pub use project::{Project, ProjectId, ProjectScope, RootSource};

/// Version of the Glasshouse binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything one Glasshouse invocation needs after startup resolution.
///
/// Holding the [`Project`] here is what makes project scope structural: any
/// component that needs a path or a database goes through this value, and the
/// only project it can reach is the active one.
#[derive(Debug, Clone)]
pub struct Runtime {
    project: Project,
    paths: RuntimePaths,
    state_dir: PathBuf,
}

impl Runtime {
    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn paths(&self) -> &RuntimePaths {
        &self.paths
    }

    /// State directory for the active project.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Directory holding Glasshouse's own files for one session.
    ///
    /// Inside the project's state directory, which is Glasshouse's alone: a
    /// harness's generated configuration lives here rather than anywhere the
    /// harness itself owns, so a Glasshouse session never edits the user's
    /// installation.
    pub fn session_dir(&self, session: &str) -> PathBuf {
        self.state_dir.join("sessions").join(session)
    }

    /// Directory holding logs for the active project.
    pub fn log_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }

    /// The project database's path, for diagnostics only.
    ///
    /// This is always `<state_dir>/glasshouse.db`; the database module derives
    /// it from the runtime and no public API accepts a different database
    /// path. Nothing here holds an open connection: each launch validates and
    /// migrates the file, then closes it until a component actually needs it.
    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join(database::DATABASE_FILE_NAME)
    }
}

/// Resolve the project and runtime paths, and create the project state
/// directory if this is the first launch for this project.
pub fn bootstrap(cli: &Cli, cwd: &Path) -> Result<Runtime> {
    let paths = RuntimePaths::resolve(cli.data_dir.as_deref(), cli.config_dir.as_deref())?;
    let project = Project::discover(cwd, cli.scope.as_deref(), cli.allow_unsafe_scope)?;

    let state_dir = paths.project_state_dir(project.id().as_str());
    create_state_dir(&state_dir).with_context(|| {
        format!(
            "could not create project state directory `{}`",
            state_dir.display()
        )
    })?;

    // First launch creates the project database; every later launch validates
    // it. After this point a successful Runtime always has a valid, bound
    // project database — the only place this project's memory may ever live.
    // The runtime is built first so the database module derives both the path
    // and the project identifier from it; nothing accepts them separately.
    let runtime = Runtime {
        project,
        paths,
        state_dir,
    };
    database::ensure_ready(&runtime)?;

    Ok(runtime)
}

/// Create the project state directory, restricted to its owner on Unix.
///
/// This directory will hold session transcripts, project memory, and
/// provider configuration — nothing world-readable belongs here. Plain
/// `create_dir_all` yields `0o777 & !umask` (typically `0o755`), so the mode
/// is set explicitly via `DirBuilder` instead.
///
/// `DirBuilder::mode` only applies to directories this call actually
/// creates: a state directory left over from before this fix, or created by
/// something else, keeps whatever permissions it already had. That is
/// accepted rather than silently assumed — this call neither widens nor
/// narrows a directory that already exists.
#[cfg(unix)]
fn create_state_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_state_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn bootstrap_isolates_state_per_project() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        let mut roots = Vec::new();
        for name in ["alpha", "beta"] {
            let root = workspace.path().join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            roots.push(root);
        }

        let mut state_dirs = Vec::new();
        for root in &roots {
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                data.path().to_str().unwrap(),
                "--config-dir",
                data.path().to_str().unwrap(),
            ])
            .unwrap();
            let runtime = bootstrap(&cli, root).unwrap();
            assert!(runtime.state_dir().is_dir());
            state_dirs.push(runtime.state_dir().to_path_buf());
        }

        assert_ne!(state_dirs[0], state_dirs[1]);
        assert!(!state_dirs[0].starts_with(&state_dirs[1]));
        assert!(!state_dirs[1].starts_with(&state_dirs[0]));
    }
}
