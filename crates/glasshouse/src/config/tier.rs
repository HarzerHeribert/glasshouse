//! [`WorkloadTier`]: how demanding a piece of work is, in the gateway's own
//! vocabulary. Glasshouse no longer classifies anything against it — the
//! 2026-09-16 ruling removed the router — but configuration still names a
//! tier in two places: a per-model ceiling (`ProviderConfig::model_ceilings`)
//! and an entitlement's allow/deny tier list (`config::entitlement`), both of
//! which are user-declared facts about a model or an account, not a routing
//! decision.
// History: design-decisions.md, "Glasshouse never decides which model is used", GH-GLASSHOUSE-ROUTING-DELETE.

pub use inference_gateway::routing::tier::WorkloadTier;
