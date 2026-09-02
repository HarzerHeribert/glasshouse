//! The one thing this build script does: give the shipped binary the same
//! main-thread stack on Windows that macOS and Linux give it by default.
//!
//! Windows reserves 1 MiB for a process's main thread; the Unix targets this
//! project ships on reserve 8. A debug build of `glasshouse` — the build every
//! shipped-binary test spawns — carries frames large enough that on
//! 2026-09-02 the first real Windows VM run since batch 86 failed 178 times
//! with *thread 'main' has overflowed its stack*, on `gateway pairs` with no
//! configuration as readily as on `route`. The fix is the linker's stack
//! reserve, not a code path: a bigger reserve costs nothing until it is
//! touched, and it changes no runtime semantics — no thread is spawned, no
//! signal or console handler moves off the main thread.
//!
//! Emitted for the binary targets only (`rustc-link-arg-bins`); the library
//! and the test harnesses run their work on threads of their own.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    // 8 MiB, the Unix default, written out so the number reads as one.
    const STACK_RESERVE_BYTES: u64 = 8 * 1024 * 1024;
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/STACK:{STACK_RESERVE_BYTES}");
    } else {
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK_RESERVE_BYTES}");
    }
}
