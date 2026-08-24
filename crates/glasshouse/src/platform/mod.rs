//! Runtime host-platform detection.
//!
//! Glasshouse must behave identically in shape on macOS, Linux, native
//! Windows, and WSL, but a handful of decisions genuinely depend on *which*
//! of these four environments is hosting the Glasshouse process itself: how
//! PTYs are opened, how signals and process termination work, how harness
//! executables are resolved, and where per-user application state lives.
//!
//! Product rule: **WSL is treated as a Linux runtime** — it uses the Unix PTY
//! and process-control code paths, Unix path semantics, and the Linux
//! per-user data directory convention. The one place WSL is *not* just Linux
//! is executable resolution ([`exec`]): a WSL `PATH` routinely contains
//! Windows interop entries (`/mnt/c/...`), and launching one of those crosses
//! into the Windows process namespace, where the Linux project-root working
//! directory Glasshouse relies on for project isolation is meaningless.
//! Windows and WSL are two distinct process namespaces, and Glasshouse must
//! never silently mix them: a WSL-hosted instance must not spawn a
//! Windows-side executable as though it were a native child process, and a
//! native-Windows-hosted instance must never reach into WSL.

pub mod exec;
pub mod paths;

use std::sync::OnceLock;

/// The operating-system environment Glasshouse's own process is running in.
///
/// This describes the *host*, not the target harness: it is what decides how
/// Glasshouse opens PTYs, resolves executables, and locates its own state,
/// regardless of which coding-agent harness the user has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostPlatform {
    MacOs,
    Linux,
    /// Linux running under Windows Subsystem for Linux. Treated as a Linux
    /// runtime everywhere except executable resolution — see the module
    /// documentation.
    Wsl,
    Windows,
}

impl HostPlatform {
    /// Detect the current host platform.
    ///
    /// The result is cached for the lifetime of the process with
    /// [`OnceLock`]: the host cannot change while Glasshouse is running, so
    /// re-reading `/proc/sys/kernel/osrelease` and environment variables on
    /// every call would just be wasted I/O.
    pub fn detect() -> HostPlatform {
        static CACHED: OnceLock<HostPlatform> = OnceLock::new();
        *CACHED.get_or_init(Self::detect_uncached)
    }

    fn detect_uncached() -> HostPlatform {
        #[cfg(windows)]
        {
            HostPlatform::Windows
        }

        #[cfg(target_os = "macos")]
        {
            HostPlatform::MacOs
        }

        #[cfg(target_os = "linux")]
        {
            // Missing or unreadable is not an error here: a container or
            // minimal chroot without /proc still counts as plain Linux.
            let osrelease =
                std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
            let has_distro_env = std::env::var_os("WSL_DISTRO_NAME").is_some();
            let has_interop_env = std::env::var_os("WSL_INTEROP").is_some();
            if is_wsl(has_distro_env, has_interop_env, &osrelease) {
                HostPlatform::Wsl
            } else {
                HostPlatform::Linux
            }
        }

        // Other Unix-like targets (the BSDs, etc.) are not a Glasshouse
        // release target, but treating them as Linux keeps the Unix PTY and
        // process-control code paths active — which are what they actually
        // have — instead of routing them through an untested fourth branch.
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            HostPlatform::Linux
        }
    }

    /// True for every platform except native Windows.
    pub fn is_unix(self) -> bool {
        !matches!(self, HostPlatform::Windows)
    }

    /// True only for native Windows (not WSL, which is a Linux runtime).
    pub fn is_windows(self) -> bool {
        matches!(self, HostPlatform::Windows)
    }
}

impl std::fmt::Display for HostPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            HostPlatform::MacOs => "macOS",
            HostPlatform::Linux => "Linux",
            HostPlatform::Wsl => "Linux (WSL)",
            HostPlatform::Windows => "Windows",
        };
        f.write_str(s)
    }
}

/// Testable core of WSL detection.
///
/// True when either interop environment variable WSL sets is present, or
/// `osrelease` mentions "microsoft" or "wsl" (case-insensitively), which is
/// how the WSL kernel identifies itself in `uname -r` / `osrelease` across
/// both WSL1 and WSL2. Factored out from [`HostPlatform::detect_uncached`] so
/// it can be exercised without a real `/proc` or environment.
// Only Linux builds ever ask this question, but the tests exercise it on every
// platform so the logic stays covered wherever CI runs.
#[cfg(any(target_os = "linux", test))]
fn is_wsl(has_distro_env: bool, has_interop_env: bool, osrelease: &str) -> bool {
    if has_distro_env || has_interop_env {
        return true;
    }
    let lower = osrelease.to_ascii_lowercase();
    lower.contains("microsoft") || lower.contains("wsl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wsl_from_env_alone() {
        assert!(is_wsl(true, false, ""));
        assert!(is_wsl(false, true, ""));
    }

    #[test]
    fn detects_wsl_from_osrelease_signature() {
        assert!(is_wsl(false, false, "5.15.90.1-microsoft-standard-WSL2"));
        assert!(is_wsl(false, false, "4.4.0-19041-Microsoft"));
        assert!(is_wsl(false, false, "5.10.0-WSL"));
        // Case-insensitivity matters: distros vary in how they capitalize this.
        assert!(is_wsl(false, false, "5.10.0-MICROSOFT-standard"));
    }

    #[test]
    fn plain_linux_osrelease_is_not_wsl() {
        assert!(!is_wsl(false, false, "6.5.0-14-generic"));
        assert!(!is_wsl(false, false, ""));
        assert!(!is_wsl(false, false, "5.4.0-alpine"));
    }

    #[test]
    fn display_labels_match_the_product_rule() {
        assert_eq!(HostPlatform::MacOs.to_string(), "macOS");
        assert_eq!(HostPlatform::Linux.to_string(), "Linux");
        assert_eq!(HostPlatform::Wsl.to_string(), "Linux (WSL)");
        assert_eq!(HostPlatform::Windows.to_string(), "Windows");
    }

    #[test]
    fn is_unix_and_is_windows_partition_correctly() {
        assert!(HostPlatform::MacOs.is_unix());
        assert!(HostPlatform::Linux.is_unix());
        assert!(HostPlatform::Wsl.is_unix());
        assert!(!HostPlatform::Windows.is_unix());

        assert!(!HostPlatform::MacOs.is_windows());
        assert!(!HostPlatform::Linux.is_windows());
        assert!(!HostPlatform::Wsl.is_windows());
        assert!(HostPlatform::Windows.is_windows());
    }

    #[test]
    fn detect_is_cached_and_consistent() {
        // detect() must not panic and must be stable across calls within one
        // process, regardless of which platform actually runs this test.
        let first = HostPlatform::detect();
        let second = HostPlatform::detect();
        assert_eq!(first, second);
    }
}
