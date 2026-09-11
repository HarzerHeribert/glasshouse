//! `glasshouse credentials` — putting a provider key where the harness and
//! its hooks can read it, taking one back out, and saying where each one
//! comes from.
//!
//! **The invariant: a credential's value never exists in this process.** It
//! is never an argument, never formatted into a message, and never part of
//! an error.
//!
//! **Since the 2026-09-11 ruling, that `String` does not exist here at all.**
//! A provider key is the gateway's, so `store` and `remove` are forwards:
//! `inference-gateway credentials set|remove --variable VAR`, with this
//! terminal's stdin inherited, so the value goes from the keyboard or the
//! pipe to the gateway without passing through this process. What stays here
//! is the part that is Glasshouse's — refusing a value on the command line
//! before anything is run, and checking the variable *name* — and `list`,
//! which reads names and sources and never a value.

use std::process::ExitCode;

use anyhow::bail;

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::secret::native::PreferNativeSecretStore;

use super::gateway_forward;

/// What `glasshouse credentials store VAR VALUE` is answered with.
///
/// It says *why* rather than only *no*, because the reason is the whole
/// point: a command line is readable by every process on the machine
/// through `ps` and is kept in the shell's history file, so a key that
/// travelled as an argument is already out whatever this command does with
/// it afterwards. The refused words themselves are counted and never read.
const ARGV_REFUSAL: &str = "the value is never an argument: command-line arguments are visible \
     to every process on this machine and are kept in your shell's history. Run `glasshouse \
     credentials store <VARIABLE>` and type the value at the prompt, or pipe it in with \
     `glasshouse credentials store <VARIABLE> --stdin`";

/// Whether `var` is a name Glasshouse will file a credential under.
///
/// Invariant: the native store only ever receives a name a shell could set —
/// non-empty, ASCII letters, digits and underscores, not starting with a
/// digit — because that is the namespace `credential_env` names and the one
/// `credentials list` reads back. Checked before any store is probed and
/// before any value is asked for, on every platform alike: macOS's Keychain
/// happened to refuse an empty account, the Secret Service and Windows
/// Credential Manager filed it (sweep 34013598118), and the difference must
/// not be the store's to decide. The name is not echoed: a value mistaken
/// for a name must not come back out in the refusal.
fn usable_variable_name(var: &str) -> anyhow::Result<()> {
    let mut chars = var.chars();
    let usable = matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    if usable {
        Ok(())
    } else {
        bail!(
            "the credential variable name is not usable: it must be non-empty, made of ASCII \
             letters, digits and underscores, and not start with a digit — give the name a \
             provider's `credential_env` lists"
        );
    }
}

/// Hand `glasshouse credentials store VAR` to the gateway.
///
/// The two checks that stay here are the two a forward cannot do later: a
/// value that arrived as a command-line argument is already out — `ps` and
/// the shell's history both have it — so it is refused **before** any
/// process is started, and a name Glasshouse would not file a credential
/// under is refused before the gateway is asked to.
///
/// `--stdin` needs nothing forwarded: the gateway reads its key from the
/// standard input this process inherits to it, which is the pipe when there
/// is one and the terminal when there is not.
pub(crate) fn store(
    runtime: &Runtime,
    var: &str,
    _from_stdin: bool,
    value_on_argv: &[String],
) -> anyhow::Result<ExitCode> {
    if !value_on_argv.is_empty() {
        bail!("{ARGV_REFUSAL}");
    }
    usable_variable_name(var)?;
    gateway_forward::credentials(runtime.paths(), "set", var)
}

/// Hand `glasshouse credentials remove VAR` to the gateway.
pub(crate) fn remove(runtime: &Runtime, var: &str) -> anyhow::Result<ExitCode> {
    usable_variable_name(var)?;
    gateway_forward::credentials(runtime.paths(), "remove", var)
}

/// Print every configured provider's credential variables and where each
/// one resolves from — **names and sources, never a value**.
///
/// Each line comes from `integrations::credential_whereabouts`, which is
/// also what `glasshouse doctor` prints, so the two commands cannot
/// describe the same credential differently.
pub(crate) fn list(runtime: &Runtime) -> anyhow::Result<()> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths())?;
    let effective = EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway);
    // The same store the gateway itself resolves through — its credential
    // file first-classed beside the platform keychain — so what this prints
    // is where a key `glasshouse credentials store` forwarded actually
    // landed, and not where Glasshouse used to keep one.
    let secrets = PreferNativeSecretStore::detect_with_file(
        inference_gateway::config::credentials_path(runtime.paths().gateway_data_dir()),
    );

    println!(
        "Credentials resolve from: {}",
        glasshouse::secret::SecretStore::describe(&secrets)
    );
    println!();

    let mut listed = 0usize;
    for name in effective.provider_names() {
        let Ok(layered) = effective.configured_provider(&name) else {
            continue;
        };
        if layered.value.credential_env.is_empty() {
            continue;
        }
        println!("  provider `{name}`");
        for var in &layered.value.credential_env {
            println!(
                "    {}",
                glasshouse::integrations::credential_whereabouts(var, &secrets)
            );
            listed += 1;
        }
    }
    if listed == 0 {
        println!("  (no configured provider names a credential variable)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal explains the mechanism, because the mechanism is the
    /// reason: a user who is only told `no` pastes the key into the next
    /// command instead.
    #[test]
    fn the_argv_refusal_names_process_listings_and_shell_history() {
        assert!(ARGV_REFUSAL.contains("visible"));
        assert!(ARGV_REFUSAL.contains("history"));
        assert!(ARGV_REFUSAL.contains("--stdin"));
    }
}
