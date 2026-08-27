//! Phase 33A — the project-local routing evidence ledger.
//!
//! **Stub, declared by the orchestrator so two workers can share `routing/`.**
//! `routing-ledger` owns this file for batch 36; `routing-score` owns the rest
//! of `routing/**` and must not edit it. The module is declared here rather
//! than by whichever worker gets there first because `routing/mod.rs` belongs
//! to the other one, and a `mod` line is not worth a cross-partition patch.
//!
//! What belongs here, per the capability map's Phase 33A:
//!
//! - an **append-oriented** record of routing observations, not only current
//!   aggregate counters (line 1329);
//! - rolling summaries computed over those raw rows rather than replacing them,
//!   so a routing decision can be audited and the aggregation recalibrated
//!   (line 1335);
//! - every aggregate carrying its own source, window, sample size, freshness
//!   and confidence (line 1339), and staying **unknown** when the sample is too
//!   small to support a decision (line 1340).
//!
//! It is also where `routing-score`'s `ObservationSource` gets a real
//! implementation, replacing `NoObservations`.
