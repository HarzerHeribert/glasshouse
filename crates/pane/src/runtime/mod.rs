//! The runtime a tool result lives in — map lines 2461 and 2465.
//! Specification: `docs/product/pane/runtime-contract.md` §2 and §3.
//!
//! Composition only. [`handles`] is the table of named live values;
//! [`preview`] renders one entry, type-directed first and size-capped second.
//! The V8 isolate that executes a model's program is a later package and
//! nothing here executes anything.

pub mod handles;
pub mod preview;
