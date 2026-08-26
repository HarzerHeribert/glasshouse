//! Durable, project-scoped memory.
//!
//! Empty on purpose: this module is declared ahead of its implementation so
//! that two concurrent workers do not both have to edit `lib.rs` to add a
//! `mod` line. The capability map's Phases 20, 22, 23 and 26 describe what
//! belongs here.
