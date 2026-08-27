//! The external control door for one project's Glasshouse — Phase 42.
//!
//! Everything a person can do from `glasshouse sessions`, `glasshouse
//! memory`, and `glasshouse checkpoint` is a local, one-shot process
//! invocation that opens the project's own database and exits. Nothing
//! outside that process can list, message, or interrupt a session while it
//! is running, because a `SessionRuntime`'s pseudo-terminal handles are
//! private to whichever process started them — there is no cross-process
//! way to reach one.
//!
//! `glasshouse api serve` is what closes that gap: a single process that
//! owns its own `SessionRuntime` and answers requests against it, plus the
//! project's memory and checkpoint stores, over a Unix domain socket. It
//! does not attach to a concurrent `glasshouse` shell or headless launch's
//! own runtime — nothing can, for the reason above — so a session started
//! outside this door is visible here (the store is shared) but not
//! controllable here (send/interrupt honestly answer `ApiError::NotLive`,
//! the same error `glasshouse sessions` itself would give for a session no
//! live process holds).
//!
//! **Why a Unix socket, not a subcommand-per-call.** A subcommand-per-call
//! ("`glasshouse api send-message ...`") is a fresh process per request, and
//! a fresh process cannot hold the `SessionRuntime` that spawning and
//! messaging a session need — every call would have to re-attach to
//! *something* long-lived regardless, so the long-lived thing might as well
//! be the door itself. A socket answers requests without needing a shell
//! already open, which a purely in-process API could not.
//!
//! **Why this is a bin-crate module, not `glasshouse::api`.** This phase's
//! packet holds `cli.rs` and `main.rs` but not `lib.rs`, which another
//! phase's partition does not own either; declaring `mod api;` from
//! `main.rs` keeps this door inside the binary that already owns
//! `run_headless`'s `Arc<Mutex<SessionRuntime>>` pattern, which this reuses,
//! without editing a file outside this package's grant. The consequence is
//! that this module is proven only by running the shipped binary — see
//! `tests/session_model.rs`'s API cluster — never by an in-process unit
//! test, which is the right proof for an external door anyway.
//!
//! **Project scope.** The socket is opened for one already-resolved
//! `Runtime`, resolved the same way every other subcommand resolves it
//! (`--scope`, or the working directory's Git root). Every handler in
//! `unix` reaches sessions through `session::api::SessionApi`, which
//! refuses a foreign session by construction — see that type's own doc
//! comment — and memory and checkpoints through `memory::ProjectMemory` and
//! `checkpoint::store::ProjectCheckpoints`, both opened against this same
//! runtime. There is no request field naming a project: the door itself is
//! the scope.
//!
//! **Authentication.** See `unix::authorize` for the mechanism and its
//! limits — a filesystem-permission and peer-credential check, not a secret.

mod protocol;

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::serve;

/// The control API needs a Unix domain socket, and Windows has no drop-in
/// equivalent (a named pipe is a different API with different authentication
/// primitives). Refusing loudly here is the honest answer until that
/// transport exists, rather than a build that silently does nothing on
/// Windows.
#[cfg(not(unix))]
pub fn serve(
    _runtime: &glasshouse::Runtime,
    _socket_override: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "glasshouse: the control API needs a Unix domain socket, which this platform does not \
         have; Windows needs a named-pipe transport that does not exist yet (Phase 42's socket \
         door is Unix-only)"
    )
}
