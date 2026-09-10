//! The standalone inference gateway: the process every provider request goes
//! through, and the binary the entitlement, subscription and routing-cost
//! controls ask.
//!
//! **Nothing here requires Glasshouse, at build time or at run time.** Pane
//! links against no gateway crate; it shells out to a sibling executable, the
//! same protocol boundary [`crate::glasshouse`] already crosses. A session
//! runs with no `glasshouse` binary anywhere on `PATH`.
//!
//! **Start or attach is decided once, from `ANTHROPIC_BASE_URL`**
//! ([`start_or_attach`]): a set variable means something upstream — Glasshouse,
//! a shell, a test — already has a gateway serving and passed its URL, so pane
//! uses it and starts nothing. An unset variable means pane is standalone and
//! owns the gateway's whole lifetime. There is no third case and no silent
//! fallback to a provider endpoint: a gateway that cannot be started is a
//! startup refusal, because a request that skipped the gateway would also skip
//! the entitlement and cost controls that are the reason it exists.

use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::contract::ServedBy;

/// How this module reaches the gateway binary. `None` never attempts a
/// command: every control reports unreachable and [`Gateway::serve`] refuses.
#[derive(Debug, Clone)]
pub enum Gateway {
    None,
    /// Shells out to this executable. `PathBuf::from("inference-gateway")`
    /// lets the OS resolve it from `PATH` (`Command::new` maps to `execvp` on
    /// a bare name); a test passes its own fake script's path instead, so no
    /// production code here performs a `PATH` lookup of its own.
    Command {
        gateway: PathBuf,
    },
}

impl Gateway {
    /// The executable this handle shells out to, for the one caller that must
    /// **stream** a command rather than wait for it (the login flow reports an
    /// authorization URL first and an outcome minutes later).
    #[must_use]
    pub fn executable(&self) -> Option<&Path> {
        match self {
            Self::None => None,
            Self::Command { gateway } => Some(gateway.as_path()),
        }
    }

    /// The one definition of reachable: found, spawned, and exited 0.
    /// `stdin`, when given, is written and then dropped -- closing that end of
    /// the pipe -- so a child reading until EOF gets exactly one message.
    pub(crate) fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Option<Vec<u8>> {
        let Gateway::Command { gateway } = self else {
            return None;
        };

        let mut command = Command::new(gateway);
        command.args(args);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::null());
        command.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = command.spawn().ok()?;
        if let Some(bytes) = stdin {
            child.stdin.take()?.write_all(bytes).ok()?;
        }
        let output = child.wait_with_output().ok()?;

        if output.status.success() {
            Some(output.stdout)
        } else {
            None
        }
    }

    /// Spawns `inference-gateway serve` and waits for its one ready line.
    ///
    /// **The read is blocking and on the caller's thread, deliberately.** The
    /// contract is that the gateway prints exactly one line as soon as it is
    /// listening, so the wait is short; doing it here rather than on a helper
    /// thread is what lets [`start_or_attach`] set the process environment
    /// while this process is still single-threaded.
    pub fn serve(&self) -> Result<Serving, ServeError> {
        let Gateway::Command { gateway } = self else {
            return Err(ServeError::Other(
                "pane cannot start: no inference gateway is configured -- pass \
                 --gateway <path>, or set ANTHROPIC_BASE_URL to attach to one \
                 already serving"
                    .to_string(),
            ));
        };

        let mut child = Command::new(gateway)
            .arg("serve")
            .arg("--listen")
            .arg("127.0.0.1:0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                let message = format!(
                    "pane cannot start: could not run the inference gateway `{}` ({e}) -- \
                     install it, pass --gateway <path>, or set ANTHROPIC_BASE_URL to \
                     attach to one already serving",
                    gateway.display()
                );
                if e.kind() == std::io::ErrorKind::NotFound {
                    ServeError::NotInstalled(message)
                } else {
                    ServeError::Other(message)
                }
            })?;

        let stdout = child
            .stdout
            .take()
            .expect("stdout was piped by the spawn above");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let ready = match reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => serde_json::from_str::<Ready>(line.trim()).ok(),
        };
        let Some(ready) = ready else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ServeError::Other(format!(
                "pane cannot start: the inference gateway `{}` did not report a \
                 listening address on its first line of output",
                gateway.display()
            )));
        };

        Ok(Serving {
            child,
            _stdout: reader,
            base_url: ready.listening,
            token: ready.token.filter(|token| !token.is_empty()),
        })
    }
}

/// Why [`Gateway::serve`] could not start a gateway. The distinction exists
/// for exactly one caller: `session::run` treats an executable that is not
/// installed as "talk to the provider directly" when nobody named one with
/// `--gateway`, and every other failure as the refusal it is.
#[derive(Debug)]
pub enum ServeError {
    /// The executable could not be found at all.
    NotInstalled(String),
    Other(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

/// The one line `inference-gateway serve` prints when it is listening.
#[derive(Debug, Deserialize)]
struct Ready {
    listening: String,
    #[serde(default)]
    token: Option<String>,
}

/// A gateway this process started, and which dies with it.
///
/// **Holding this value is what keeps the gateway alive**, so a caller binds
/// it for as long as the session lasts. [`Drop`] kills the child rather than
/// only closing its stdin: stdin closing is the gateway's own shutdown signal
/// and both are sent, but only the kill is not contingent on the child
/// reading.
pub struct Serving {
    child: Child,
    /// Held, not read. The gateway prints one line and then keeps this pipe
    /// open; dropping the reader would close pane's end of it.
    _stdout: BufReader<ChildStdout>,
    base_url: String,
    token: Option<String>,
}

impl Serving {
    /// The URL this gateway is listening on -- what `ANTHROPIC_BASE_URL` is
    /// set to for the rest of the process.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The bearer the gateway minted for this session, when it minted one.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        // Closing stdin is the contract's polite shutdown; the kill is the one
        // that does not depend on the child ever reading it.
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Attaches to a gateway that is already serving, or starts one.
///
/// Returns `Ok(None)` when `ANTHROPIC_BASE_URL` is already set: pane is hosted
/// and the URL it was handed is the answer. Otherwise it starts one, points
/// [`crate::wire::base_url`] and [`crate::wire`]'s credential header at it via
/// the environment, and returns the handle whose lifetime is the gateway's.
///
/// # Safety of the environment write
///
/// `std::env::set_var` is unsound beside another thread reading the
/// environment. The only caller is `session::run`, before it spawns the
/// interrupt watcher or starts the live UI, and [`Gateway::serve`] itself
/// starts no thread -- so this process is single-threaded at the write.
pub fn start_or_attach(gateway: &Gateway) -> Result<Option<Serving>, ServeError> {
    if std::env::var("ANTHROPIC_BASE_URL").is_ok_and(|value| !value.is_empty()) {
        return Ok(None);
    }

    let serving = gateway.serve()?;
    // SAFETY: single-threaded at this point -- see the doc comment above.
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", serving.base_url());
        if let Some(token) = serving.token() {
            std::env::set_var("ANTHROPIC_AUTH_TOKEN", token);
        }
    }
    Ok(Some(serving))
}

// ---------------------------------------------------------------------
// Which entitlement served each request, and what it cost
// ---------------------------------------------------------------------

/// One row of `inference-gateway routing-cost --json`. Only the columns this
/// module needs are declared; every other key is ignored by `serde_json`
/// without any attribute here.
#[derive(Debug, Deserialize)]
struct ObservationRow {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    quota_context: Option<String>,
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cached_input_tokens: Option<u64>,
}

/// Fills a [`ServedBy`] from `inference-gateway routing-cost --json --since
/// <since>`. **The row used is the last model observation printed** (excluding
/// local context-firewall bookkeeping): rows arrive ascending by
/// `observed_at`, and the last row at or after `since` is the one closest to
/// the request this call is answering for.
///
/// Absent is not zero: no gateway, a launch failure, a non-zero exit, or an
/// empty window all produce [`ServedBy::default`], whose `is_known` is
/// `false` -- never a `ServedBy` with token fields defaulted to zero.
pub fn served_by(gateway: &Gateway, since: SystemTime) -> ServedBy {
    let since_secs = unix_secs(since).to_string();
    let Some(stdout) = gateway.run(&["routing-cost", "--json", "--since", &since_secs], None)
    else {
        return ServedBy::default();
    };

    let text = String::from_utf8_lossy(&stdout);
    let row = text
        .lines()
        .filter_map(|line| serde_json::from_str::<ObservationRow>(line.trim()).ok())
        // Tool/firewall bookkeeping is not a model request or an entitlement.
        .rfind(|row| {
            !(row.provider.as_deref() == Some("glasshouse")
                && row.model.as_deref() == Some("context-firewall"))
        });

    match row {
        Some(row) => ServedBy {
            provider: row.provider,
            model: row.model,
            route: row.route,
            quota_context: row.quota_context,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_input_tokens: row.cached_input_tokens,
        },
        None => ServedBy::default(),
    }
}

fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}
