//! Non-interactive version probing for harness executables.
//!
//! Detection must never risk an interactive prompt. `claude --version` and
//! friends are expected to print a line and exit, but Glasshouse cannot trust
//! that every harness (or a future one) behaves that way, so this module
//! treats every probe as a hostile-until-proven-otherwise child process:
//!
//! - stdin is `/dev/null`-equivalent (`Stdio::null()`), so a program that
//!   unexpectedly waits on a prompt sees immediate EOF instead of hanging on
//!   a terminal that will never answer.
//! - stdout/stderr are piped and captured, never inherited — probe output
//!   must not leak into whatever terminal UI is hosting Glasshouse.
//! - A real wall-clock timeout is enforced by polling `try_wait`, since
//!   `std::process::Command` has no built-in one. On timeout the child is
//!   killed and reaped so probing never leaves a zombie behind.
//! - Captured output is bounded, so a misbehaving program that floods its
//!   pipes cannot exhaust Glasshouse's memory.

use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::Project;
use crate::platform::exec::{LaunchError, ResolvedExecutable};

/// Default timeout for a version probe.
///
/// Five seconds is generous for a well-behaved `--version` invocation (which
/// should be near-instant) while still bounding how long `glasshouse doctor`
/// can be stalled by a single misbehaving or slow-starting executable.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on how much combined stdout+stderr a probe will keep.
///
/// Version output is a handful of bytes in every real case; this exists only
/// to stop a pathological program (or one that was mistakenly matched as a
/// harness executable) from being able to grow an unbounded buffer.
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// How often the timeout loop polls the child for completion.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Why a version probe could not produce a result.
///
/// Note what is deliberately *not* here: there is no variant for "no version
/// string found in the output" — that is not a probe failure, it is a
/// successful probe that answers `Ok(None)` (see [`probe_version`]).
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The child process could not even be started.
    #[error("failed to start `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// The child did not exit before the deadline. It has already been
    /// killed and reaped by the time this is returned.
    #[error("`{program}` did not exit within {timeout:?}")]
    Timeout { program: String, timeout: Duration },

    /// The OS could not report whether the child had exited. This is rare
    /// (typically a platform-level wait(2) failure) and is surfaced rather
    /// than silently treated as success or failure. The child has already
    /// been killed and reaped by the time this is returned.
    #[error("could not determine whether `{program}` had exited: {source}")]
    WaitFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// The resolved executable's launch command could not be safely
    /// assembled — e.g. a `.cmd`/`.bat` shim's path or an argument being
    /// probed contains a `cmd.exe` metacharacter (see
    /// [`crate::platform::exec::LaunchError`]). This surfaces as a clean
    /// probe failure rather than a panic or a silent skip.
    #[error("cannot launch `{program}` to probe its version: {source}")]
    InvalidLaunchCommand {
        program: String,
        #[source]
        source: LaunchError,
    },
}

/// A parsed `major.minor.patch` version, alongside the raw text it was
/// extracted from (kept for display, since harness `--version` output is not
/// standardised and users benefit from seeing exactly what the tool printed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    raw: String,
}

impl Version {
    pub fn major(&self) -> u64 {
        self.major
    }

    pub fn minor(&self) -> u64 {
        self.minor
    }

    pub fn patch(&self) -> u64 {
        self.patch
    }

    /// The exact substring this version was parsed from, e.g. `"1.2.3"`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Whether this version is at least `min`, comparing `(major, minor,
    /// patch)` lexicographically (standard semver-ish ordering; no
    /// pre-release/build metadata handling, since harness `--version` output
    /// does not reliably provide any).
    pub fn satisfies_minimum(&self, min: &Version) -> bool {
        self.as_tuple() >= min.as_tuple()
    }

    fn as_tuple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_tuple().cmp(&other.as_tuple())
    }
}

/// Parse the first `\d+(\.\d+)?(\.\d+)?` sequence in `text`.
///
/// Harness `--version` output is not standardised (e.g. `claude` prints
/// something like `1.2.3 (Claude Code)`, others print `vX.Y`, others print a
/// bare number). This deliberately does not try to be clever about picking
/// "the most version-looking" number if the very first digit run in the
/// output is not actually the version — e.g. a build/date stamp preceding
/// the real version would be misread. That is an accepted limitation of a
/// simple, predictable first-match rule applied to genuinely unstandardised
/// output; a false positive here only ever affects an advisory report, never
/// which binary gets launched.
pub fn parse_version(text: &str) -> Option<Version> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some((major, mut pos)) = take_number(&chars, i) {
            let mut minor = 0u64;
            let mut patch = 0u64;
            if pos < chars.len()
                && chars[pos] == '.'
                && let Some((m, next_pos)) = take_number(&chars, pos + 1)
            {
                minor = m;
                pos = next_pos;
                if pos < chars.len()
                    && chars[pos] == '.'
                    && let Some((p, next_pos)) = take_number(&chars, pos + 1)
                {
                    patch = p;
                    pos = next_pos;
                }
            }
            let raw: String = chars[i..pos].iter().collect();
            return Some(Version {
                major,
                minor,
                patch,
                raw,
            });
        }
        i += 1;
    }
    None
}

/// Consume a run of ASCII digits starting at `start`. Returns the parsed
/// value and the index just past the run, or `None` if `start` is not a
/// digit. A run too long to fit `u64` saturates rather than failing the
/// whole parse — that is a pathological input, not something worth aborting
/// detection over.
fn take_number(chars: &[char], start: usize) -> Option<(u64, usize)> {
    if start >= chars.len() || !chars[start].is_ascii_digit() {
        return None;
    }
    let mut end = start;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    let text: String = chars[start..end].iter().collect();
    let value = text.parse::<u64>().unwrap_or(u64::MAX);
    Some((value, end))
}

/// Run `exe arg` non-interactively and try to parse a version from its
/// combined stdout+stderr.
///
/// Returns `Ok(None)` (not an error) when the process ran and exited but no
/// version-looking text was found in its output — that is a successful probe
/// with an inconclusive answer, which callers should treat as "unknown", not
/// as a failure.
pub fn probe_version(
    exe: &ResolvedExecutable,
    arg: &str,
    project: &Project,
    timeout: Duration,
) -> Result<Option<Version>, ProbeError> {
    let (program, args) =
        exe.spawn_command([arg])
            .map_err(|source| ProbeError::InvalidLaunchCommand {
                program: exe.path().display().to_string(),
                source,
            })?;
    let program_display = display_program(&program);

    let mut child = std::process::Command::new(&program)
        .args(&args)
        // Version probes are harness processes too. Derive their cwd from
        // the active project rather than inheriting Glasshouse's process cwd.
        .current_dir(project.display_root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ProbeError::Spawn {
            program: program_display.clone(),
            source,
        })?;

    // Draining is not optional: a child that fills its stdout/stderr pipe
    // buffer blocks forever on write(2) if nobody is reading the other end,
    // which would turn a "timeout" into a real hang during the kill/reap
    // below. Each stream gets its own thread so a chatty stderr can't starve
    // stdout (or vice versa).
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_handle = std::thread::spawn(move || drain_capped(stdout, MAX_CAPTURE_BYTES));
    let stderr_handle = std::thread::spawn(move || drain_capped(stderr, MAX_CAPTURE_BYTES));

    let deadline = Instant::now() + timeout;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break WaitOutcome::Exited,
            Ok(None) => {
                if Instant::now() >= deadline {
                    break WaitOutcome::TimedOut;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(source) => break WaitOutcome::Errored(source),
        }
    };

    match outcome {
        WaitOutcome::TimedOut => {
            // Best-effort: the process is already misbehaving relative to
            // our expectations, so a failure to kill/wait it here is not
            // something a caller of `probe_version` could act on either way.
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            Err(ProbeError::Timeout {
                program: program_display,
                timeout,
            })
        }
        WaitOutcome::Errored(source) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            Err(ProbeError::WaitFailed {
                program: program_display,
                source,
            })
        }
        WaitOutcome::Exited => {
            // `try_wait` returning `Ok(Some(_))` means the child has already
            // been reaped (waitpid consumed it) — no zombie is left behind.
            let stdout_bytes = stdout_handle.join().unwrap_or_default();
            let stderr_bytes = stderr_handle.join().unwrap_or_default();
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&stdout_bytes),
                String::from_utf8_lossy(&stderr_bytes)
            );
            Ok(parse_version(&combined))
        }
    }
}

enum WaitOutcome {
    Exited,
    TimedOut,
    Errored(std::io::Error),
}

fn display_program(program: &Path) -> String {
    program.display().to_string()
}

/// Read `source` to end-of-stream on the calling thread, keeping only the
/// first `cap` bytes but continuing to consume (and discard) anything past
/// that. Consuming past the cap, rather than stopping, is what keeps a
/// chatty child from blocking on a full pipe once capture stops.
fn drain_capped<R: Read>(source: Option<R>, cap: usize) -> Vec<u8> {
    let mut reader = match source {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < cap {
                    let remaining = cap - buf.len();
                    buf.extend_from_slice(&chunk[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::exec;

    fn test_project() -> (tempfile::TempDir, Project) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let project = Project::discover(tmp.path(), None, false).unwrap();
        (tmp, project)
    }

    // --- parse_version -----------------------------------------------------

    #[test]
    fn parses_full_semver_with_trailing_text() {
        let v = parse_version("1.2.3 (Claude Code)").unwrap();
        assert_eq!((v.major(), v.minor(), v.patch()), (1, 2, 3));
        assert_eq!(v.raw(), "1.2.3");
    }

    #[test]
    fn parses_major_minor_only() {
        let v = parse_version("codex-cli v0.42\n").unwrap();
        assert_eq!((v.major(), v.minor(), v.patch()), (0, 42, 0));
        assert_eq!(v.raw(), "0.42");
    }

    #[test]
    fn parses_bare_major() {
        let v = parse_version("version 15").unwrap();
        assert_eq!((v.major(), v.minor(), v.patch()), (15, 0, 0));
        assert_eq!(v.raw(), "15");
    }

    #[test]
    fn parses_leading_v_prefixed_version() {
        let v = parse_version("v2.0.11").unwrap();
        assert_eq!((v.major(), v.minor(), v.patch()), (2, 0, 11));
    }

    #[test]
    fn no_digits_at_all_returns_none() {
        assert!(parse_version("error: not logged in").is_none());
        assert!(parse_version("").is_none());
    }

    #[test]
    fn first_digit_run_wins_even_if_not_the_real_version() {
        // Documented limitation: this is not "smart", it is predictable.
        let v = parse_version("build 20240101 version 1.2.3").unwrap();
        assert_eq!(v.raw(), "20240101");
    }

    // --- ordering / satisfies_minimum --------------------------------------

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
            raw: format!("{major}.{minor}.{patch}"),
        }
    }

    #[test]
    fn ordering_compares_numerically_not_lexically() {
        assert!(v(1, 9, 0) < v(1, 10, 0));
        assert!(v(2, 0, 0) > v(1, 99, 99));
        assert_eq!(v(1, 2, 3), v(1, 2, 3));
    }

    #[test]
    fn satisfies_minimum_is_inclusive() {
        let min = v(1, 2, 0);
        assert!(v(1, 2, 0).satisfies_minimum(&min));
        assert!(v(1, 2, 1).satisfies_minimum(&min));
        assert!(v(2, 0, 0).satisfies_minimum(&min));
        assert!(!v(1, 1, 9).satisfies_minimum(&min));
    }

    // --- probe_version -------------------------------------------------

    #[test]
    fn probes_a_real_version_printing_binary() {
        // Prefer `git`, fall back to `cargo`; skip gracefully if neither is
        // on PATH rather than failing the whole suite over environment
        // shape.
        let Some(exe) = exec::resolve("git")
            .ok()
            .or_else(|| exec::resolve("cargo").ok())
        else {
            eprintln!("skipping: neither `git` nor `cargo` is on PATH");
            return;
        };
        let (_guard, project) = test_project();
        let result = probe_version(&exe, "--version", &project, DEFAULT_PROBE_TIMEOUT).unwrap();
        assert!(
            result.is_some(),
            "expected a parsed version from --version output"
        );
    }

    #[test]
    fn timeout_kills_a_hanging_process_promptly() {
        let Ok(exe) = exec::resolve("sleep") else {
            eprintln!("skipping: `sleep` is not on PATH");
            return;
        };
        let (_guard, project) = test_project();
        let start = Instant::now();
        let result = probe_version(&exe, "30", &project, Duration::from_secs(1));
        let elapsed = start.elapsed();

        assert!(matches!(result, Err(ProbeError::Timeout { .. })));
        // Well under the 30s the child was asked to sleep for.
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout took too long: {elapsed:?}"
        );
    }

    #[test]
    fn null_stdin_prevents_hanging_on_a_stdin_reading_program() {
        // `cat -` explicitly reads stdin. With stdin wired to null, it should
        // see immediate EOF and exit right away instead of blocking forever
        // waiting for input that will never arrive.
        let Ok(exe) = exec::resolve("cat") else {
            eprintln!("skipping: `cat` is not on PATH");
            return;
        };
        let (_guard, project) = test_project();
        let start = Instant::now();
        let result = probe_version(&exe, "-", &project, Duration::from_secs(5));
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "cat - should exit cleanly: {result:?}");
        assert!(
            elapsed < Duration::from_secs(3),
            "cat - took too long, stdin may not be null: {elapsed:?}"
        );
    }

    /// Name of the file the probe below looks for, placed only in the project
    /// root. Deliberately not a path: the probe checks for it *relatively*,
    /// which is what makes finding it equivalent to "my working directory is
    /// that project root".
    const CWD_MARKER: &str = "glasshouse-probe-cwd-marker";

    /// A probe that reports a version only when it can see [`CWD_MARKER`] in
    /// its own working directory.
    ///
    /// An earlier version compared path *strings* — the child's `pwd` against
    /// its own `$0/..`. That passed on Unix and failed on `windows-latest`,
    /// because the two sides are not spelled the same way there: the script
    /// path arrives canonicalized (potentially with a verbatim `\\?\` prefix)
    /// while `%CD%` is the plain form, and a GitHub Windows runner's temporary
    /// directory can additionally appear under an 8.3 short name
    /// (`RUNNER~1`). Both spellings denote the same directory, so the
    /// comparison was wrong even though the behaviour under test was right.
    ///
    /// Looking for a relative filename asks the filesystem instead of
    /// comparing text, so it cannot be fooled by either spelling — and it is a
    /// stronger statement of the property anyway: the child really is *in*
    /// that directory, not merely naming it.
    #[cfg(unix)]
    fn install_cwd_checking_probe(bin_dir: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cwd-version-probe");
        std::fs::write(
            &path,
            format!("#!/bin/sh\n[ -e ./{CWD_MARKER} ] || exit 23\necho 9.8.7\n"),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    /// Windows counterpart of the probe above; see its doc comment for why
    /// this checks for a file rather than comparing paths.
    #[cfg(windows)]
    fn install_cwd_checking_probe(bin_dir: &Path) -> std::path::PathBuf {
        let path = bin_dir.join("cwd-version-probe.cmd");
        std::fs::write(
            &path,
            format!("@echo off\r\nif not exist \"{CWD_MARKER}\" exit /b 23\r\necho 9.8.7\r\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn version_probe_child_starts_in_the_active_project_root() {
        let (guard, project) = test_project();
        let bin_dir = guard.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        let path = install_cwd_checking_probe(&bin_dir);
        let exe = exec::resolve_explicit(&path).unwrap();

        // The marker goes in the project root and nowhere else, so the probe
        // can only find it by actually having been started there. Note it is
        // *not* placed in `bin_dir`, where the probe executable itself lives:
        // a child that inherited the caller's directory, or that ran beside
        // its own binary, finds nothing and exits 23.
        std::fs::write(project.display_root().join(CWD_MARKER), b"").unwrap();

        let probed = probe_version(&exe, "--version", &project, DEFAULT_PROBE_TIMEOUT).unwrap();

        // A bare "expected Some, got None" here says nothing about *why* the
        // child could not see the marker, and this test only runs on the
        // platform whose behaviour is in question — so when it fails, it has
        // to be able to explain itself from the CI log alone. Re-running the
        // same invocation with captured output costs nothing on the passing
        // path and is the difference between one diagnostic round trip and
        // several.
        let Some(version) = probed else {
            let (program, args) = exe.spawn_command(["--version"]).expect("safe command line");
            let output = std::process::Command::new(&program)
                .args(&args)
                .current_dir(project.display_root())
                .output()
                .expect("re-run the probe for diagnostics");
            panic!(
                "the probe reported no version, so its working directory was not the \
                 project root.\n  \
                 program:        {}\n  \
                 args:           {:?}\n  \
                 requested cwd:  {}\n  \
                 canonical root: {}\n  \
                 marker present: {}\n  \
                 exit status:    {:?}\n  \
                 stdout:         {:?}\n  \
                 stderr:         {:?}",
                program.display(),
                args,
                project.display_root().display(),
                project.root().display(),
                project.display_root().join(CWD_MARKER).is_file(),
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        };
        assert_eq!(
            (version.major(), version.minor(), version.patch()),
            (9, 8, 7)
        );
    }

    /// The other half of the claim: with no marker to find, the probe reports
    /// nothing. Without this, the test above could pass for a probe that
    /// printed a version unconditionally.
    #[test]
    fn the_cwd_checking_probe_reports_nothing_when_the_marker_is_absent() {
        let (guard, project) = test_project();
        let bin_dir = guard.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        let path = install_cwd_checking_probe(&bin_dir);
        let exe = exec::resolve_explicit(&path).unwrap();

        // Deliberately no marker written.
        let probed = probe_version(&exe, "--version", &project, DEFAULT_PROBE_TIMEOUT).unwrap();
        assert!(
            probed.is_none(),
            "the probe must not report a version without the marker: {probed:?}"
        );
    }
}
