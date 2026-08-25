//! Running a real native harness as a Glasshouse session.
//!
//! A session is a real installed harness in a real pseudo-terminal, started
//! inside the active project root and never anywhere else. Opening one has
//! two halves, and this module holds both:
//!
//! - [`select`] decides *which* harness and *which* executable, refusing
//!   ambiguity rather than guessing;
//! - [`attach`] hands the terminal to it and stays out of the way.
//!
//! [`store`] holds the third: Glasshouse's own durable record of the sessions
//! in this project, kept independently of whatever session files the harness
//! writes for itself.
//!
//! Selecting and attaching both go through [`crate::launch::HarnessLaunch`],
//! the only sanctioned way to start a harness: it derives the child's working
//! directory from the active project and offers no way to override it.

pub mod attach;
pub mod lifecycle;
pub mod runtime;
pub mod select;
pub mod store;

pub use attach::attach;
pub use lifecycle::{lifecycle_for, may_apply};
pub use runtime::{LiveSession, RuntimeError, Scrollback, SessionRuntime};
pub use select::{ExecutableSource, HarnessSelection, SelectionError, select};
pub use store::{
    NewSession, ProjectSessions, ResumableSession, SessionDisposition, SessionId, SessionLifecycle,
    SessionPresentation, SessionRecord, SessionRole, SessionStore, SessionStoreError,
};
