//! The macOS applier: a seatbelt profile generated from a compiled
//! [`Profile`] and entered before `exec` — map line 2455, specification
//! `docs/product/pane/sandbox-grants.md` §3.
//!
//! The invariant: **the generated text can only ever be narrower than the
//! profile it came from.** Every `allow` term here is either fixed platform
//! machinery (the loader's own paths, the executable roots) or the project
//! root, and the project-root terms are emitted only when
//! [`Profile::check`] says the root is reachable — so there is no input, at
//! render time or at spawn time, that adds a path the profile does not
//! already permit. The text is a value: [`profile_text`] produces it with no
//! process anywhere in sight, which is what lets a test assert on it.
//!
//! macOS is the one platform whose OS layer can express §2's pattern
//! language exactly — `(subpath …)` for a directory glob and `(regex …)`
//! for an extension filter. This applier does **not** reach that exactness
//! yet, and [`Regime`] says so rather than implying it; see that type's
//! documentation for the reason, which is a missing accessor on `Profile`
//! and not a property of seatbelt.

use super::profile::{Access, Profile};
use std::fmt;
use std::path::Path;

/// Where a tool process may be executed from, and read, regardless of what
/// the settings document says.
///
/// These are not grants over project data and they are not derived from
/// `permissions`: they are the minimum a process needs to exist at all —
/// `.claude/settings.json` has no pattern kind that names an interpreter, so
/// deriving them from it is not possible even in principle. A path here
/// carries no user content; the never-grantable set of §4 is disjoint from
/// it, and `(deny default)` refuses everything else.
const EXECUTABLE_ROOTS: [&str; 6] = [
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
    "/usr/local/bin",
    "/opt/homebrew/bin",
];

/// The loader's own reach. Read-only, and system-owned on every macOS
/// install: dyld resolves the shared cache and the executable's libraries
/// before the process gets to run a single instruction of its own.
const LOADER_READ_ROOTS: [&str; 10] = [
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/private/etc",
    "/System",
    "/Library",
    "/opt/homebrew",
    "/private/var/db/dyld",
    "/private/var/db/timezone",
];

/// Character devices a process may read. Each is a device, not a file, and
/// none of them carries project or user data.
const DEVICE_READS: [&str; 8] = [
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
];

/// Character devices a process may write. `/dev/dtracehelper` is opened for
/// write by the loader itself on every exec; refusing it costs a denial
/// record on every spawn and buys nothing.
const DEVICE_WRITES: [&str; 5] = [
    "/dev/null",
    "/dev/dtracehelper",
    "/dev/tty",
    "/dev/stdout",
    "/dev/stderr",
];

/// Which enforcement this applier actually achieved, so a coarser regime is
/// stated rather than implied (§3's closing sentence, applied to macOS).
///
/// Seatbelt can express every pattern in §2's table. **This applier cannot
/// reach that**, and the reason is one missing capability on the merged
/// producer rather than anything about the operating system: [`Profile`]
/// keeps its compiled rules private and publishes no way to enumerate them,
/// so a rule naming a path outside the project root — §3's own
/// `(literal "/etc/passwd")` — cannot be rendered, and neither can a `deny`
/// rule naming a path inside it. The first case makes the OS layer *narrower*
/// than the profile, which is safe and merely inconvenient; the second makes
/// it *wider*, and the in-process [`Profile::check`] is what holds the line
/// there. [`Regime::path_rules`] carries the count so a session can say how
/// many rules went unrendered instead of claiming an exactness it has not
/// got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// `(deny default)` with the project root granted and §1.5's `.claude`
    /// write-deny rendered. `path_rules` is how many `Read`/`Write`/`Edit`
    /// rules the profile compiled, every one of which is enforced by
    /// [`Profile::check`] and none of which reaches the OS layer.
    ProjectRootOnly { path_rules: usize },
}

impl Regime {
    /// The sentence a session prints at start-up. It names the coarseness,
    /// because §3 requires pane to state it rather than imply exactness.
    pub fn describe(self) -> String {
        match self {
            Regime::ProjectRootOnly { path_rules } => format!(
                "seatbelt: deny by default, the project root the only readable and writable root, no network. \
                 The OS layer is directory-granular; {path_rules} path rule(s) from `.claude/settings.json` are enforced by pane's own pre-call check alone."
            ),
        }
    }
}

impl fmt::Display for Regime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// The regime [`profile_text`] achieves for `profile`.
pub fn regime(profile: &Profile) -> Regime {
    Regime::ProjectRootOnly {
        path_rules: profile.rule_count(),
    }
}

/// Renders the seatbelt profile for `profile`, as text.
///
/// Deterministic in its single argument: the same profile renders the same
/// bytes on every call, and nothing in the environment, the argv or the
/// command line about to be spawned reaches this function. That is
/// mechanical rather than asserted — there is no other parameter — and it is
/// how invariant §1.1 survives contact with the process-spawning half of the
/// sandbox.
pub fn profile_text(profile: &Profile) -> String {
    let root = display(profile.root());
    let mut out = String::new();
    out.push_str("(version 1)\n");
    out.push_str("(deny default)\n");

    // Metadata, and only metadata. Resolving any path at all requires
    // stat-ing its parents, so a deny-default profile that withholds this
    // cannot exec. It discloses existence and size, never content.
    out.push_str("(allow file-read-metadata)\n");
    // `/` itself is read by the loader on every exec.
    out.push_str("(allow file-read* (literal \"/\"))\n");

    out.push_str("(allow process-exec*");
    for path in EXECUTABLE_ROOTS {
        out.push_str(&format!(" (subpath {})", quote(path)));
    }
    out.push_str(")\n");
    out.push_str("(allow process-fork)\n");
    out.push_str("(allow signal (target self))\n");
    out.push_str("(allow sysctl-read)\n");
    out.push_str("(allow mach-lookup)\n");

    out.push_str("(allow file-read*");
    for path in LOADER_READ_ROOTS {
        out.push_str(&format!(" (subpath {})", quote(path)));
    }
    for path in DEVICE_READS {
        out.push_str(&format!(" (literal {})", quote(path)));
    }
    out.push_str(")\n");
    out.push_str("(allow file-write-data");
    for path in DEVICE_WRITES {
        out.push_str(&format!(" (literal {})", quote(path)));
    }
    out.push_str(")\n");
    out.push_str("(allow file-ioctl (literal \"/dev/tty\") (literal \"/dev/dtracehelper\"))\n");

    // The profile decides, not this function. A settings document whose
    // `deny` list covers the project root produces no grant here at all,
    // which is the only reason these two lines are conditional.
    if profile
        .check("read", Access::Read, &profile.root().join(READ_PROBE))
        .is_ok()
    {
        out.push_str(&format!("(allow file-read* (subpath {}))\n", quote(&root)));
    }
    if profile
        .check("write", Access::Write, &profile.root().join(WRITE_PROBE))
        .is_ok()
    {
        out.push_str(&format!("(allow file-write* (subpath {}))\n", quote(&root)));
    }

    // §1.5: `.claude/` lives inside the writable root, so a program that
    // could write it could widen the profile it was derived from. Emitted
    // when — and only when — the profile agrees it is unwritable, so the two
    // layers cannot disagree about it.
    let dot_claude = profile.root().join(".claude");
    if profile
        .check("write", Access::Write, &dot_claude.join(WRITE_PROBE))
        .is_err()
    {
        out.push_str(&format!(
            "(deny file-write* (subpath {}))\n",
            quote(&display(&dot_claude))
        ));
    }

    // §4.1. Unconditional in practice, because `grants_network` is: no
    // `permissions` pattern names a host, a port or a protocol, so there is
    // nothing a document could say that would reach this branch.
    if !profile.grants_network() {
        out.push_str("(deny network*)\n");
    }
    out
}

/// A file name no settings pattern is expected to spell, used to ask the
/// profile about a directory rather than about a file that happens to exist.
const READ_PROBE: &str = ".pane-sandbox-read-probe";
const WRITE_PROBE: &str = ".pane-sandbox-write-probe";

/// Quotes a path for a seatbelt profile literal.
///
/// Backslash and double quote are the only two characters the profile
/// language's string syntax reserves, and escaping them here is what stops a
/// project directory whose name contains one from closing the term early and
/// being read as more profile — the injection this whole file would
/// otherwise be one `mkdir` away from.
fn quote(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for ch in path.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Applies `profile`'s seatbelt profile to `command`, to take effect in the
/// child between `fork` and `exec`.
///
/// The text is rendered here from the profile rather than accepted as an
/// argument, and that is the whole of requirement 5: a caller holding a
/// `Profile` cannot hand this function a wider policy than the one the
/// profile implies, because there is no parameter through which to hand it
/// anything.
///
/// The `CString` is built before the fork. Everything the child does after
/// that is one call into libSystem with a pointer that already exists.
#[cfg(target_os = "macos")]
pub fn confine(profile: &Profile, command: &mut std::process::Command) -> std::io::Result<()> {
    use std::ffi::{CString, c_char};
    use std::os::unix::process::CommandExt;

    let text = CString::new(profile_text(profile))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // SAFETY: `pre_exec` runs in the forked child before `exec`. The only
    // call it makes is `sandbox_init` on a `CString` allocated in the parent,
    // so nothing here allocates and nothing touches this process's state.
    unsafe {
        command.pre_exec(move || {
            let mut error: *mut c_char = std::ptr::null_mut();
            let applied = sandbox_init(text.as_ptr(), 0, &mut error);
            if applied != 0 {
                return Err(std::io::Error::other("sandbox_init refused the profile"));
            }
            Ok(())
        });
    }
    Ok(())
}

// libSystem's seatbelt entry point. `flags` is `0`, which is what makes the
// first argument a profile *string* rather than the name of one of Apple's
// built-in profiles.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sandbox_init(
        profile: *const std::ffi::c_char,
        flags: u64,
        errorbuf: *mut *mut std::ffi::c_char,
    ) -> i32;
}
