//! The Windows applier: a restricted token and an AppContainer — map line
//! 2455, specification `docs/product/pane/sandbox-grants.md` §3.
//!
//! **The job object is not a sandbox and grants nothing.** Glasshouse
//! already creates one (`crates/glasshouse/src/pty/process.rs`, with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) and `crates/glasshouse/src/pty/mod.rs`
//! says it outright — *"this is structure within the sanctioned harness API,
//! not a sandbox."* It is a **lifetime** primitive: it guarantees the process
//! tree dies with pane. 61D reuses it for that and for nothing else, and the
//! map line's phrase "Windows job objects" must not be read as naming the
//! grant mechanism, because it does not.
//!
//! The two primitives that do grant something are
//! [`RestrictedToken`]'s — `CreateRestrictedToken` with `WRITE_RESTRICTED`
//! and deny-only SIDs removes the user's own write reach — and
//! [`AppContainer`]'s: a capability SID the project directory's ACL is
//! extended to admit, and nothing else. An AppContainer declaring no
//! `internetClient` capability has no network, which is how §4.1 is enforced
//! here.
//!
//! Not expressible: ACLs are per-object, so an extension-filtered glob gets
//! Linux's treatment — a directory-granular ACE, with the filter exact in
//! [`Profile::check`] alone. Case-insensitivity is the platform's, and
//! `Profile`'s matcher already makes the same decision on every host rather
//! than three different ones.

use super::profile::{Access, Profile};
use std::fmt;
use std::path::{Path, PathBuf};

/// Which enforcement this applier achieved.
///
/// **Unverified everywhere as of this package**: the `pane` CI job is Ubuntu
/// and macOS, there is no Windows cell, and no assertion below has ever run
/// on Windows. The variants are what the applier reports, not what anyone
/// has watched it do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// A `WRITE_RESTRICTED` token inside an AppContainer whose capability
    /// SID the project's ACL admits. No `internetClient`, so no network.
    RestrictedTokenAndAppContainer,
    /// The token restriction applied and the AppContainer did not — the
    /// user's write reach is removed, but the process is not isolated into
    /// a container and the network is still there.
    RestrictedTokenOnly,
    /// Neither applied.
    Unconfined,
}

impl Regime {
    /// The sentence a session prints at start-up.
    pub fn describe(self) -> String {
        match self {
            Regime::RestrictedTokenAndAppContainer =>
                "restricted token (WRITE_RESTRICTED, deny-only SIDs) inside an AppContainer with no internetClient capability, \
                 the project directory's ACL extended to that capability SID alone. ACLs are per-object: an extension-filtered \
                 pattern is enforced at directory granularity here and exactly by pane's own pre-call check."
                    .to_string(),
            Regime::RestrictedTokenOnly =>
                "restricted token (WRITE_RESTRICTED, deny-only SIDs) and no AppContainer: the user's own write reach is removed, \
                 but this process is not container-isolated and its network is not removed."
                    .to_string(),
            Regime::Unconfined =>
                "no OS-level confinement: neither the restricted token nor the AppContainer could be created, \
                 so pane's own pre-call check is the only thing between a tool and the filesystem."
                    .to_string(),
        }
    }

    /// Whether the network is removed by the OS layer. Only the
    /// AppContainer does that here — a restricted token has no bearing on
    /// sockets at all.
    pub fn removes_network(self) -> bool {
        matches!(self, Regime::RestrictedTokenAndAppContainer)
    }
}

impl fmt::Display for Regime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// Which access the AppContainer's capability SID is granted, and where.
///
/// A value, so the derivation is assertable on a host that cannot run a
/// single Win32 call — which is every host this package was written on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclGrants {
    /// Directories whose ACL admits the capability SID for read and execute.
    pub read_only: Vec<PathBuf>,
    /// Directories whose ACL admits it for read, write and delete.
    pub read_write: Vec<PathBuf>,
    /// Whether the AppContainer declares `internetClient`. Always `false`:
    /// no `permissions` pattern names a host, a port or a protocol, so a
    /// network capability would have to be invented (§4.1).
    pub internet_client: bool,
}

/// Derives the ACL grants `profile` implies.
///
/// The project root, and nothing else. Every system directory an
/// AppContainer needs is already readable by `ALL APPLICATION PACKAGES`, so
/// there is nothing to add for them and nothing here that could be widened
/// into them. `.claude/` is carved back to read-only inside a writable root
/// (§1.5), and both decisions are [`Profile::check`]'s rather than this
/// function's.
pub fn acl_grants(profile: &Profile) -> AclGrants {
    let root = profile.root().to_path_buf();
    let mut read_only = Vec::new();
    let mut read_write = Vec::new();
    if grants(profile, Access::Write, &root) {
        read_write.push(root.clone());
        let dot_claude = root.join(".claude");
        if !grants(profile, Access::Write, &dot_claude) {
            read_only.push(dot_claude);
        }
    } else if grants(profile, Access::Read, &root) {
        read_only.push(root);
    }
    AclGrants {
        read_only,
        read_write,
        internet_client: profile.grants_network(),
    }
}

/// The AppContainer profile name for a project root.
///
/// Derived from the root alone and stable across sessions, so a container
/// created once is reused rather than accumulating one profile per launch.
/// It is not a secret and carries no path: `CreateAppContainerProfile`
/// caps the name at 64 UTF-16 code units, and a project path is routinely
/// longer than that.
pub fn container_name(profile: &Profile) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in profile.root().to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("Glasshouse.Pane.{hash:016x}")
}

/// A file name no settings pattern is expected to spell, so the question put
/// to the profile is about the directory rather than about whichever file
/// happens to exist in it.
const PROBE: &str = ".pane-sandbox-probe";

fn grants(profile: &Profile, access: Access, directory: &Path) -> bool {
    profile
        .check(access.as_str(), access, &directory.join(PROBE))
        .is_ok()
}

#[cfg(target_os = "windows")]
pub use platform::{AppContainer, RestrictedToken, grant_project_acl, regime};

#[cfg(target_os = "windows")]
mod platform {
    use super::{AclGrants, Regime, acl_grants, container_name};
    use crate::sandbox::profile::Profile;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW,
        SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
    };
    use windows_sys::Win32::Security::{
        ACL, CreateRestrictedToken, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_ALL_ACCESS, WRITE_RESTRICTED,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// A `WRITE_RESTRICTED` token: the process keeps its read identity and
    /// loses its write reach everywhere the restricting SID list does not
    /// name.
    ///
    /// `WRITE_RESTRICTED` with an empty restricting-SID list is the strongest
    /// form — every write access check must be satisfied by a SID in a list
    /// that has none — and that is deliberate: the AppContainer's capability
    /// SID, granted on the project ACL alone, is what puts the project back.
    pub struct RestrictedToken(HANDLE);

    impl RestrictedToken {
        /// Derives the restricted token from this process's own token.
        pub fn derive() -> io::Result<Self> {
            let mut source: HANDLE = std::ptr::null_mut();
            // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs
            // no close, and `source` is a live out-parameter.
            let opened =
                unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut source) };
            if opened == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut restricted: HANDLE = std::ptr::null_mut();
            // SAFETY: every count is zero and every list pointer is null,
            // which is the documented way to ask for `WRITE_RESTRICTED` with
            // no disabling, deleting or restricting SIDs of its own.
            let created = unsafe {
                CreateRestrictedToken(
                    source,
                    WRITE_RESTRICTED,
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    &mut restricted,
                )
            };
            // SAFETY: `source` is the handle `OpenProcessToken` returned.
            unsafe { CloseHandle(source) };
            if created == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(restricted))
        }

        /// The raw handle, for a `CreateProcessAsUserW` the spawn path owns.
        pub fn handle(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for RestrictedToken {
        fn drop(&mut self) {
            // SAFETY: `self.0` is the handle `CreateRestrictedToken` returned
            // and is closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// An AppContainer profile and its capability SID.
    ///
    /// Created with **no** capabilities. `internetClient` is the one that
    /// would give the process a network, and §4.1 is why it is absent rather
    /// than present and disabled.
    pub struct AppContainer {
        sid: PSID,
    }

    impl AppContainer {
        /// Creates — or reuses — the container for `profile`'s project root.
        pub fn create(profile: &Profile) -> io::Result<Self> {
            let name = wide(&container_name(profile));
            let mut sid: PSID = std::ptr::null_mut();
            // SAFETY: `name` outlives the call; a null capability pointer
            // with a zero count is the documented way to ask for a container
            // with no capabilities at all.
            let created = unsafe {
                CreateAppContainerProfile(
                    name.as_ptr(),
                    name.as_ptr(),
                    name.as_ptr(),
                    std::ptr::null(),
                    0,
                    &mut sid,
                )
            };
            if created < 0 {
                // `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)` is the ordinary
                // second launch, not a failure: the profile is per project
                // root and outlives the session that made it.
                let already = (created as u32) == (0x8007_0000 | ERROR_ALREADY_EXISTS);
                if !already {
                    return Err(io::Error::from_raw_os_error(created));
                }
                // SAFETY: `name` outlives the call and `sid` is a live
                // out-parameter.
                let derived =
                    unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
                if derived < 0 {
                    return Err(io::Error::from_raw_os_error(derived));
                }
            }
            Ok(Self { sid })
        }

        /// The capability SID, for the `SECURITY_CAPABILITIES` a
        /// `STARTUPINFOEXW` carries.
        pub fn sid(&self) -> PSID {
            self.sid
        }
    }

    impl Drop for AppContainer {
        fn drop(&mut self) {
            if !self.sid.is_null() {
                // SAFETY: the SID was allocated by the container APIs, which
                // document `FreeSid`'s `LocalFree` as its release.
                unsafe { LocalFree(self.sid) };
            }
        }
    }

    /// Extends the project directory's ACL to admit `container`'s SID, and
    /// admits nothing else.
    ///
    /// The grants come from [`acl_grants`], so the profile decides which
    /// directories appear and whether the writable one is writable. A
    /// protected DACL is set so an inherited ACE from a parent directory
    /// cannot put back what this removes.
    pub fn grant_project_acl(profile: &Profile, container: &AppContainer) -> io::Result<()> {
        let grants: AclGrants = acl_grants(profile);
        for (paths, rights) in [
            (&grants.read_only, READ_RIGHTS),
            (&grants.read_write, READ_WRITE_RIGHTS),
        ] {
            for path in paths {
                let entry = EXPLICIT_ACCESS_W {
                    grfAccessPermissions: rights,
                    grfAccessMode: GRANT_ACCESS,
                    grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                    Trustee: TRUSTEE_W {
                        pMultipleTrustee: std::ptr::null_mut(),
                        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                        TrusteeForm: TRUSTEE_IS_SID,
                        TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                        ptstrName: container.sid().cast(),
                    },
                };
                let mut acl: *mut ACL = std::ptr::null_mut();
                // SAFETY: one entry, whose address and count agree; a null
                // existing-ACL pointer builds a fresh ACL from that entry.
                let built = unsafe { SetEntriesInAclW(1, &entry, std::ptr::null_mut(), &mut acl) };
                if built != 0 {
                    return Err(io::Error::from_raw_os_error(built as i32));
                }
                let mut wide_path = wide(&path.to_string_lossy());
                // SAFETY: `wide_path` and `acl` outlive the call.
                let applied = unsafe {
                    SetNamedSecurityInfoW(
                        wide_path.as_mut_ptr(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        acl,
                        std::ptr::null_mut(),
                    )
                };
                // SAFETY: `acl` was allocated by `SetEntriesInAclW`.
                unsafe { LocalFree(acl.cast()) };
                if applied != 0 {
                    return Err(io::Error::from_raw_os_error(applied as i32));
                }
            }
        }
        Ok(())
    }

    /// `FILE_GENERIC_READ | FILE_GENERIC_EXECUTE`.
    const READ_RIGHTS: u32 = 0x0012_00A9;
    /// The above plus `FILE_GENERIC_WRITE` and `DELETE`.
    const READ_WRITE_RIGHTS: u32 = 0x001F_01FF;

    /// What this host can actually enforce for `profile`.
    ///
    /// Both primitives are attempted and dropped again: the answer is what
    /// the operating system permitted just now, not what the code intends.
    pub fn regime(profile: &Profile) -> Regime {
        if RestrictedToken::derive().is_err() {
            return Regime::Unconfined;
        }
        match AppContainer::create(profile) {
            Ok(_) => Regime::RestrictedTokenAndAppContainer,
            Err(_) => Regime::RestrictedTokenOnly,
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}
