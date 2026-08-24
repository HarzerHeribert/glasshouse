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
//! Both go through [`crate::launch::HarnessLaunch`], the only sanctioned way
//! to start a harness: it derives the child's working directory from the
//! active project and offers no way to override it.

pub mod attach;
pub mod select;

pub use attach::attach;
pub use select::{ExecutableSource, HarnessSelection, SelectionError, select};
