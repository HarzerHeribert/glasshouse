//! Portable session checkpoints.
//!
//! Empty on purpose: declared ahead of its implementation so that concurrent
//! workers never have to edit `lib.rs` to add a `mod` line. This is the third
//! time the trick has been used and the second time it was reinvented one
//! level down by a team lead without being told — see the measurements ledger.
//! The capability map's Phase 19 describes what belongs here.
