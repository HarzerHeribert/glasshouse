//! The normalized Glasshouse lifecycle-event stream.
//!
//! Empty on purpose: this module is declared ahead of its implementation so
//! that two concurrent workers do not both have to edit `lib.rs` to add a
//! `mod` line. The capability map's Phases 12, 13 and 45 describe what
//! belongs here.
