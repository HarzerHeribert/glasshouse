//! The sandbox every tool a pane program can call runs under — map lines
//! 2455–2457, specification `docs/product/pane/sandbox-grants.md`.
//!
//! Composition only. [`profile`] compiles `.claude/settings.json`'s
//! `permissions` into a profile and answers every pre-call path question;
//! the platform appliers are its siblings, one per operating system.

pub mod linux;
pub mod macos;
pub mod profile;
pub mod windows;
