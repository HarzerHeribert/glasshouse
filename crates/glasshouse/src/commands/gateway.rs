//! `commands::gateway` -- moved verbatim from `main.rs` (Phase 59 decomposition).

/// `glasshouse entitlements` — map line 1972's inspectable view of the pool.
///
/// A pure function returning a `String`, like [`status_report`] and
/// `resources_report`: what it prints is testable without a terminal, which
/// is the only reason a view of this kind can be asserted at all.
///
/// # Every configured entitlement, including the ones nothing measured
///
/// The rows come from the **configuration**, not from the telemetry and not
/// from the sessions table, so an account no reading describes still gets a
/// row and reads `unknown` on the facets it has no reading for. An
/// entitlement missing from the view because nothing had measured it is the
/// exact failure 56A step 2's Cluster E discipline exists to prevent: unknown
/// is a rendered word, never full, never empty, never a number.
///
/// # Why `served` is *not* one of those unknowns
///
/// The four telemetry facets are `unknown` when nobody looked. `served` is
/// different in kind: this function **does** look, at every session row this
/// project recorded, and an account with no rows has a *measured* zero. That
/// is `SessionRecord::observed_compactions`' distinction, and rendering
/// "nothing recorded" where the sessions table is empty rather than `unknown`
/// is what keeps the two apart.
///
/// # Names, never credentials
///
/// An entitlement is named by its `[entitlements.<name>]` key and described
/// by its kind and vendor. Its `credential` is a `config::SecretRef` and this
/// function never touches it — nothing here opens a secret store, and there
/// is no branch on which this view could print a value.
/// The gateway's translation table as compiled: every ordered wire-protocol
/// pair with its status, then each codec's refused and ignored fields and its
/// prompt-cache and effort dispositions.
///
/// Reads nothing but the binary: `translate::pairs()` and
/// `translate::field_rows()` are static tables, so this opens no file, reads
/// no configuration, and resolves no secret.
pub(crate) fn gateway_pairs_report() -> String {
    use std::fmt::Write as _;

    use glasshouse::gateway::translate::{self, CacheDisposition, EffortDisposition};

    let mut out = String::new();
    let _ = writeln!(out, "PAIRS");
    let _ = writeln!(out, "=====");
    for pair in translate::pairs() {
        match pair.refusal() {
            None => {
                let _ = writeln!(out, "{} -> {}: supported", pair.from, pair.to);
            }
            Some(reason) => {
                let _ = writeln!(out, "{} -> {}: refused ({reason})", pair.from, pair.to);
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "FIELDS");
    let _ = writeln!(out, "======");
    for protocol in translate::PROTOCOLS {
        let _ = writeln!(out, "{protocol}");
        match translate::field_rows(protocol) {
            None => {
                let _ = writeln!(out, "  no codec");
            }
            Some(rows) => {
                for (field, reason) in rows.refused {
                    let _ = writeln!(out, "  refuses {field}: {reason}");
                }
                for field in rows.ignored {
                    let _ = writeln!(out, "  ignores {field}");
                }
                match rows.cache {
                    Some(CacheDisposition::Carried { field, note }) => {
                        let _ = writeln!(out, "  cache: carried under {field} ({note})");
                    }
                    Some(CacheDisposition::Stripped(reason)) => {
                        let _ = writeln!(out, "  cache: stripped: {reason}");
                    }
                    None => {
                        let _ = writeln!(out, "  cache: not applicable");
                    }
                }
                match rows.effort {
                    Some(EffortDisposition::Carried { field, note }) => {
                        let _ = writeln!(out, "  effort: carried under {field} ({note})");
                    }
                    Some(EffortDisposition::Stripped(reason)) => {
                        let _ = writeln!(out, "  effort: stripped: {reason}");
                    }
                    None => {
                        let _ = writeln!(out, "  effort: not applicable");
                    }
                }
            }
        }
    }
    out
}
