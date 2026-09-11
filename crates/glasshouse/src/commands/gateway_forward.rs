//! Handing a command to the `inference-gateway` binary.
//!
//! User ruling, 2026-09-11: subscription, account and broker state is the
//! gateway's, and Glasshouse neither owns nor manages it. Four Glasshouse
//! commands used to *write* that state — `subscriptions connect`, `login`,
//! `logout` and `adopt-binary` — and two used to write a provider key
//! (`credentials store`, `credentials remove`). They still exist, because a
//! person's fingers and a harness's scripts already know them, but each is
//! now a **forward**: the gateway's binary does the work, in this terminal,
//! and its exit status is this process's.
//!
//! Three things this deliberately does not do. It never reads the child's
//! output — a login flow's stdout can carry a code, and copying it would
//! make Glasshouse a place a credential passes through. It never puts a
//! value on the child's command line — `credentials set` reads its key from
//! the inherited stdin, so the key goes from the terminal to the gateway
//! without this process seeing it. And it never "helps" by falling back to
//! doing the work itself when the binary is missing: a missing gateway is a
//! refusal that names what to install, not a quiet return to owning the
//! state again.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};

use glasshouse::RuntimePaths;

/// Names the gateway executable outright — the override a test uses, and the
/// escape hatch for an install that is not on `PATH`.
pub(crate) const BINARY_ENV: &str = "INFERENCE_GATEWAY_BIN";

/// The executable's name on this platform.
fn binary_name() -> &'static str {
    if cfg!(windows) {
        "inference-gateway.exe"
    } else {
        "inference-gateway"
    }
}

/// Where the gateway binary is: `INFERENCE_GATEWAY_BIN`, else beside this
/// executable, else `PATH`.
///
/// Beside-this-executable before `PATH` because the two binaries ship
/// together: a user who unpacked both into a tools directory and put nothing
/// on `PATH` is the normal portable install this project supports, and
/// finding a *different* gateway on `PATH` in that case would forward the
/// user's accounts to the wrong store.
pub(crate) fn binary() -> PathBuf {
    if let Some(explicit) = std::env::var_os(BINARY_ENV)
        && !explicit.is_empty()
    {
        return PathBuf::from(explicit);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let beside = dir.join(binary_name());
        if beside.is_file() {
            return beside;
        }
    }
    PathBuf::from(binary_name())
}

/// Run the gateway with `args`, inheriting this terminal, and answer with
/// its exit status.
///
/// `subject` is the half-sentence the one stderr line begins with — *what*
/// is the gateway's — so a person who typed a Glasshouse command and saw a
/// gateway one run learns why rather than only that.
pub(crate) fn forward(paths: &RuntimePaths, subject: &str, args: &[OsString]) -> Result<ExitCode> {
    let binary = binary();
    eprintln!(
        "glasshouse: {subject} is the gateway's; forwarding to `{} {}`",
        binary.display(),
        render(args)
    );

    let mut command = Command::new(&binary);
    command
        .args(args)
        // The gateway resolves its own store from these two when they are
        // set. Passing what this process resolved is what keeps a display
        // (`glasshouse subscriptions status`) and a write (the forward)
        // looking at one store rather than two that agree only by default.
        .env(
            "INFERENCE_GATEWAY_CONFIG",
            paths.gateway_config_path().as_os_str(),
        )
        .env(
            "INFERENCE_GATEWAY_DATA_DIR",
            paths.gateway_data_dir().as_os_str(),
        )
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = command.status().with_context(|| {
        format!(
            "could not run the inference gateway `{}`. Install it beside `glasshouse`, put it \
             on PATH, or name it with {BINARY_ENV}",
            binary.display()
        )
    })?;
    Ok(exit_code(status))
}

/// The child's status as this process's exit code.
///
/// A child killed by a signal has no code of its own; `FAILURE` is the
/// honest answer, and the caller's shell already saw the child's own
/// reporting on the inherited stderr.
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code.rem_euclid(256)).unwrap_or(1)),
        None => ExitCode::FAILURE,
    }
}

/// The arguments as one readable line. Every argument a forward carries is a
/// name — a provider, an account, a variable, a path — so this can be shown;
/// no forward ever carries a value.
fn render(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `glasshouse subscriptions connect|login|logout` → the gateway's own.
pub(crate) fn subscriptions(
    paths: &RuntimePaths,
    verb: &str,
    provider: &str,
    entitlement: &str,
    json: bool,
) -> Result<ExitCode> {
    let mut args: Vec<OsString> = vec![
        OsString::from("subscriptions"),
        OsString::from(verb),
        OsString::from(provider),
        OsString::from("--entitlement"),
        OsString::from(entitlement),
    ];
    if json {
        args.push(OsString::from("--json"));
    }
    forward(paths, "connecting a subscription account", &args)
}

/// `glasshouse subscriptions adopt-binary <path>` → the gateway's own.
pub(crate) fn adopt_binary(paths: &RuntimePaths, path: &OsStr) -> Result<ExitCode> {
    let args = vec![
        OsString::from("subscriptions"),
        OsString::from("adopt-binary"),
        path.to_os_string(),
    ];
    forward(paths, "the subscription broker's executable", &args)
}

/// `glasshouse credentials store|remove VAR` → the gateway's own
/// `credentials set|remove --variable VAR`.
///
/// The key never reaches this process: `set` reads it from the stdin this
/// forward inherited.
pub(crate) fn credentials(paths: &RuntimePaths, verb: &str, var: &str) -> Result<ExitCode> {
    let args = vec![
        OsString::from("credentials"),
        OsString::from(verb),
        OsString::from("--variable"),
        OsString::from(var),
    ];
    forward(paths, "a provider key", &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `INFERENCE_GATEWAY_BIN` names the binary outright, and the bare name
    /// is the fallback — the two ends of [`binary`]'s three-step search.
    ///
    /// The environment variable is set and removed inside this one test, the
    /// same discipline `paths.rs`'s own environment test keeps: these run on
    /// parallel threads of one process.
    #[test]
    fn the_gateway_binary_is_named_by_the_environment_or_found_by_name() {
        // SAFETY: set and removed within this single test, which never runs
        // concurrently with itself.
        unsafe {
            std::env::set_var(BINARY_ENV, "/somewhere/else/inference-gateway");
        }
        let named = binary();
        unsafe {
            std::env::remove_var(BINARY_ENV);
        }
        assert_eq!(named, PathBuf::from("/somewhere/else/inference-gateway"));
    }

    /// The one stderr line names the binary and every argument, and the
    /// arguments are names only.
    #[test]
    fn the_rendered_command_is_names_only() {
        let args = vec![
            OsString::from("subscriptions"),
            OsString::from("connect"),
            OsString::from("anthropic"),
            OsString::from("--entitlement"),
            OsString::from("claude-a"),
        ];
        assert_eq!(
            render(&args),
            "subscriptions connect anthropic --entitlement claude-a"
        );
    }
}
