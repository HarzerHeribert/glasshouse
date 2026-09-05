//! `commands::gateway` -- moved verbatim from `main.rs` (Phase 59 decomposition).

/// The gateway's translation table as compiled: every ordered wire-protocol
/// pair with its status, then each codec's refused and ignored fields and its
/// prompt-cache and effort dispositions.
///
/// Reads nothing but the binary: `translate::pairs()` and
/// `translate::field_rows()` are static tables, so this opens no file, reads
/// no configuration, and resolves no secret.
///
/// History: design-decisions.md, "Trims: commands module docs", gateway_pairs_report.
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
