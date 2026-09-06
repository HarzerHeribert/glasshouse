//! `glasshouse credentials` — putting a provider key where the harness and
//! its hooks can read it, taking one back out, and saying where each one
//! comes from.
//!
//! **The invariant: a credential's value exists here as one `String`, read
//! from a terminal with echo off or from one line of standard input, handed
//! straight to [`NativeSecretStore::store`], and printed by nothing.** It is
//! never an argument, never formatted into a message, and never part of an
//! error — every failure below reports the store's own fixed classification,
//! which `secret::native`'s `classify` chose by the error's variant alone.
//!
//! Why the verb has to exist at all: on macOS an item's access control list
//! names the program that created it, and Glasshouse asks for no
//! authorization dialog by design (`secret/native.rs`,
//! `silence_authorization_dialogs`), because `doctor` and the session
//! launcher must never block on one. An item filed by hand with `security
//! add-generic-password` is therefore unreadable by Glasshouse and reads as
//! absent. Only an item this binary wrote is one this binary can resolve, so
//! writing one has to be something the binary itself does.

use anyhow::{Context as _, bail};

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::secret::SecretStore as _;
use glasshouse::secret::native::{
    Deletion, NativeStoreError, PreferNativeSecretStore, Presence, SERVICE,
    os_credential_for_variable,
};

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

/// Read one credential's value and file it under `var` in the native store.
///
/// The order is deliberate: the store is probed **before** the value is
/// asked for, so a platform that cannot keep it never prompts for it.
pub(crate) fn store(var: &str, from_stdin: bool, value_on_argv: &[String]) -> anyhow::Result<()> {
    if !value_on_argv.is_empty() {
        bail!("{ARGV_REFUSAL}");
    }

    let secrets = PreferNativeSecretStore::detect();
    let native = match secrets.native() {
        Ok(native) => native,
        // The same sentence every notice already gives on a platform with
        // no store, through the same type, so the two cannot drift.
        Err(reason) => bail!("{}", NativeStoreError::Unavailable(reason)),
    };

    // Asked **before** the value is, so a name Glasshouse cannot write is
    // never a prompt the user answers for nothing.
    let reference = os_credential_for_variable(var);
    if native.presence(&reference) == Presence::Refused {
        bail!("{}", foreign_item_refusal(var));
    }

    let value = if from_stdin {
        one_line_of_stdin()?
    } else {
        prompt_for_value(var)?
    };
    if value.is_empty() {
        // Not stored rather than stored empty: an empty credential resolves
        // to a key the provider rejects, and it would shadow the
        // environment variable that might otherwise have worked.
        zeroize(value);
        bail!("no value was given, so nothing was stored");
    }

    let outcome = native.store(&reference, &value);
    zeroize(value);
    outcome?;

    println!("Stored {var} in {}.", native.describe());
    Ok(())
}

/// Delete `var`'s credential from the native store.
pub(crate) fn remove(var: &str) -> anyhow::Result<()> {
    let secrets = PreferNativeSecretStore::detect();
    let native = match secrets.native() {
        Ok(native) => native,
        Err(reason) => bail!("{}", NativeStoreError::Unavailable(reason)),
    };

    let reference = os_credential_for_variable(var);
    if native.presence(&reference) == Presence::Refused {
        bail!("{}", foreign_item_refusal(var));
    }

    match native.delete(&reference)? {
        Deletion::Removed => println!("Removed {var} from {}.", native.describe()),
        Deletion::AlreadyAbsent => {
            println!("{var} was not in {}; nothing to remove.", native.describe());
        }
    }
    Ok(())
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
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let secrets = PreferNativeSecretStore::detect();

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

/// What to do about an item already filed under this name that Glasshouse
/// can neither read nor replace.
///
/// **Every user who followed the instruction this package replaced is in
/// this state**, and it is the one state whose fix is not "run the verb".
/// Measured on 2026-09-06 against a real hand-filed item: read, overwrite
/// and delete were all three refused, and for one reason — every one of them
/// begins with a read, macOS answers a read of an item whose access control
/// list does not name this program with an authorization request, and
/// `secret::native`'s `silence_authorization_dialogs` has turned that
/// request into an error. The program that created the item is the one that
/// can remove it.
///
/// Checked before the value is asked for, so nobody types a key into a
/// prompt that was never going to be able to keep it.
fn foreign_item_refusal(var: &str) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "there is already an item filed as `{SERVICE}`/`{var}` that Glasshouse can neither \
             read nor replace: on macOS an item's access control list names the program that \
             created it, and this one was created by something else. Remove it with `security \
             delete-generic-password -s {SERVICE} -a {var}` and run this command again"
        )
    } else {
        format!(
            "there is already an item filed as `{SERVICE}`/`{var}` that Glasshouse can neither \
             read nor replace. Remove it with the credential tool that created it and run this \
             command again"
        )
    }
}

/// One line from standard input, with the line terminator removed and
/// nothing else.
///
/// `read_line` stops at the first `\n`, which is what "at most one line"
/// means here: a second line on the pipe is never read and never stored.
fn one_line_of_stdin() -> anyhow::Result<String> {
    use std::io::BufRead as _;

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("could not read the value from standard input")?;
    Ok(without_line_terminator(line))
}

/// One trailing `\n`, and the `\r` that may precede it, and **nothing
/// else**.
///
/// `echo`, a heredoc and a file all add a newline that is not part of the
/// key, so keeping it would store a credential no provider accepts — and
/// the failure would arrive much later, as an authentication error, with
/// nothing pointing back here. Trimming further would be the opposite
/// mistake: spaces inside a value are the user's, and quietly removing them
/// would store a key that is not the one they piped.
fn without_line_terminator(mut line: String) -> String {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    line
}

/// Overwrite a value's bytes before its allocation goes back to the
/// allocator.
///
/// **Stated honestly, because the alternative is a false sense of one:**
/// this is not a guarantee that no copy of the key remains in this process.
/// The `String` may have reallocated while it was being read, `keyring`
/// copies the value on its way to the platform, and nothing here can reach
/// either. What it does control is the buffer this module is still holding
/// at the moment the command reports its result, which is also the buffer a
/// core dump taken then would contain.
///
/// The writes are volatile so the optimiser cannot drop them as stores to
/// memory nothing reads again — which is exactly what they are.
fn zeroize(mut value: String) {
    // SAFETY: every byte is overwritten with `0`, which is valid UTF-8, so
    // the `String`'s invariant holds at every point in the loop and after
    // it. The length is not changed and no byte outside the string is
    // touched.
    unsafe {
        for byte in value.as_mut_vec().iter_mut() {
            std::ptr::write_volatile(byte, 0);
        }
    }
    drop(value);
}

/// Ask for the value at a terminal that does not echo it.
#[cfg(unix)]
fn prompt_for_value(var: &str) -> anyhow::Result<String> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    use std::os::fd::AsRawFd as _;

    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        // Refused rather than read: without a terminal there is no echo to
        // turn off, and reading anyway would take a value from a pipe the
        // user did not say was one.
        bail!(
            "there is no terminal to prompt on. Pipe the value in with `glasshouse credentials \
             store {var} --stdin` instead"
        );
    }

    // The prompt goes to stderr so that redirecting stdout keeps the
    // command's answer and not the question.
    let mut stderr = std::io::stderr();
    write!(stderr, "Value for {var} (not shown): ")?;
    stderr.flush()?;

    let hidden = echo::Off::acquire(stdin.as_raw_fd())?;
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    drop(hidden);
    // The Return that ended the line was not echoed either, so without this
    // the next thing printed would land on the prompt line.
    let _ = writeln!(std::io::stderr());
    read.context("could not read the value from the terminal")?;

    Ok(without_line_terminator(line))
}

/// No prompt here yet, and it says so rather than reading the value from
/// somewhere the user did not offer it.
///
/// Turning echo off on a Windows console means `SetConsoleMode` without
/// `ENABLE_ECHO_INPUT`, which needs `windows-sys`'
/// `Win32_System_Console` feature; that is a manifest change and this
/// package does not make one. `--stdin` is the whole supported path on
/// Windows, and `integrations::store_credential_instruction` says so in the
/// same words on that platform.
#[cfg(not(unix))]
fn prompt_for_value(var: &str) -> anyhow::Result<String> {
    bail!(
        "Glasshouse cannot turn off this terminal's echo on this platform, so it will not read a \
         credential from it. Pipe the value in with `glasshouse credentials store {var} --stdin` \
         instead"
    )
}

/// Turning the terminal's echo off for the length of one prompt, and
/// **giving it back on every way out of this process**.
///
/// Three exits and three restorations. An ordinary return and an unwinding
/// panic both run [`Drop`]. An interrupt runs neither: at a prompt nothing
/// owns the terminal in `shutdown`'s sense, so a signal means *leave
/// immediately* and `force_exit` calls [`std::process::exit`], which runs no
/// destructor. That third path is what [`glasshouse::shutdown::on_forced_exit`]
/// exists for — and a Ctrl-C at a hidden prompt that handed the user back a
/// shell with echo off would be exactly the kind of defect it was built to
/// prevent.
#[cfg(unix)]
mod echo {
    use std::io;
    use std::os::fd::RawFd;

    use glasshouse::shutdown::{self, ForcedExitGuard};

    pub(super) struct Off {
        fd: RawFd,
        saved: libc::termios,
        /// Unregisters the forced-exit restoration when this guard drops,
        /// so the callback can never outlive the prompt it belongs to.
        _forced: ForcedExitGuard,
    }

    impl Off {
        pub(super) fn acquire(fd: RawFd) -> io::Result<Self> {
            let saved = attributes(fd)?;
            let mut hidden = saved;
            hidden.c_lflag &= !libc::ECHO;
            apply(fd, &hidden)?;
            let forced = shutdown::on_forced_exit(move || {
                let _ = apply(fd, &saved);
            });
            Ok(Self {
                fd,
                saved,
                _forced: forced,
            })
        }
    }

    impl Drop for Off {
        fn drop(&mut self) {
            let _ = apply(self.fd, &self.saved);
        }
    }

    fn attributes(fd: RawFd) -> io::Result<libc::termios> {
        let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `tcgetattr` writes a whole `struct termios` through the
        // pointer it is given and returns non-zero without writing on
        // failure, which is the only path that leaves the value
        // uninitialised — and that path returns before `assume_init`.
        let rc = unsafe { libc::tcgetattr(fd, term.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `tcgetattr` returned success, so it initialised the whole
        // struct.
        Ok(unsafe { term.assume_init() })
    }

    /// `TCSAFLUSH` on both the way in and the way out, and it is load
    /// bearing in both directions: going in it discards anything typed
    /// ahead, which would otherwise be read as the start of the key; coming
    /// out it discards anything typed *after* the key, which would
    /// otherwise be handed to the shell as a command.
    fn apply(fd: RawFd, term: &libc::termios) -> io::Result<()> {
        // SAFETY: a C call taking a file descriptor this process owns and a
        // pointer to a fully initialised `struct termios` that outlives the
        // call.
        let rc = unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, term) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value a `--stdin` pipe carries is the key, and the newline the
    /// pipe adds is not part of it.
    #[test]
    fn one_line_loses_its_terminator_and_nothing_else() {
        assert_eq!(
            without_line_terminator("gsk-abc123\n".to_owned()),
            "gsk-abc123"
        ); // glasshouse:not-a-secret
        assert_eq!(
            without_line_terminator("gsk-abc123\r\n".to_owned()), // glasshouse:not-a-secret
            "gsk-abc123"
        );
        assert_eq!(
            without_line_terminator("gsk-abc123".to_owned()),
            "gsk-abc123"
        ); // glasshouse:not-a-secret
    }

    /// Only the terminator. A key with a space in it is a key with a space
    /// in it, and a store that quietly trimmed one would file something the
    /// user never typed.
    #[test]
    fn nothing_inside_the_value_is_trimmed() {
        assert_eq!(
            without_line_terminator("  spaces  matter  \n".to_owned()),
            "  spaces  matter  "
        );
        assert_eq!(
            without_line_terminator("two\nlines\n".to_owned()),
            "two\nlines",
            "only the final terminator is removed"
        );
    }

    /// The one refusal a user cannot act on by re-reading the instruction:
    /// it has to name the command that clears the item, because Glasshouse
    /// cannot clear it itself.
    #[test]
    fn the_foreign_item_refusal_names_the_command_that_removes_the_item() {
        let refusal = foreign_item_refusal("GROQ_API_KEY");
        assert!(refusal.contains("GROQ_API_KEY"), "{refusal}");
        assert!(
            refusal.contains("neither read nor replace"),
            "it must say why the verb cannot fix it: {refusal}"
        );
        assert!(refusal.contains("Remove it"), "{refusal}");
        if cfg!(target_os = "macos") {
            assert!(
                refusal.contains(
                    "security delete-generic-password -s glasshouse -a \
                     GROQ_API_KEY"
                ),
                "the exact command, so it can be pasted: {refusal}"
            );
        }
    }

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
