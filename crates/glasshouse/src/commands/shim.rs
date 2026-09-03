//! `commands::shim` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use std::path::Path;
use std::process::ExitCode;

use glasshouse::platform::HostPlatform;
use glasshouse::shim::{self, ShimRequest};

/// Generate one file that `exec`s `glasshouse run <harness> --profile
/// <name>`, forwarding its own arguments.
///
/// The generated file is the entire mechanism — see [`glasshouse::shim`]'s
/// module doc. This function only resolves *this* executable's own path and
/// the host platform; [`shim::generate`] is the only thing that writes
/// anything, and it writes exactly one file, inside `dir` and nowhere else.
pub(crate) fn run_shim(
    harness: &str,
    profile: &str,
    dir: &Path,
    name: Option<&str>,
    force: bool,
) -> anyhow::Result<ExitCode> {
    let glasshouse_exe = std::env::current_exe().map_err(|err| {
        anyhow::anyhow!("could not determine the Glasshouse executable's own path: {err}")
    })?;
    let request = ShimRequest {
        harness,
        profile,
        glasshouse_exe: &glasshouse_exe,
        dir,
        name,
        force,
    };

    match shim::generate(HostPlatform::detect(), &request) {
        Ok(path) => {
            println!("wrote {}", path.display());
            println!(
                "deleting that file is all it takes to remove the shim; Glasshouse writes \
                 nothing else on its behalf."
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            eprintln!("glasshouse: {err}");
            Ok(ExitCode::FAILURE)
        }
    }
}
