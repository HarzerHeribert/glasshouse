//! The provenance header — map line 1986. One compact block prefixed to a
//! reduced result; nothing is ever added to a passthrough result. Extended
//! by Phase 57B (map lines 1997-2003) with an optional second line stating
//! what the semantic stage did, when it was attempted at all.

/// What the header's semantic line states — present only when
/// [`crate::firewall::SemanticOutcome`] is, i.e. the stage was actually
/// attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticProvenance {
    pub applied: bool,
    /// How many of the candidates the semantic stage considered survived —
    /// `0` when `applied` is `false`, since nothing from this attempt was
    /// applied.
    pub kept: usize,
    /// How many candidates the semantic stage was given to decide over.
    pub considered: usize,
    /// The bypass reason's stable name, when `applied` is `false`.
    pub reason: Option<String>,
    /// The reducer's own identity — `"{provider} {model}"` — present
    /// whenever a call actually completed with a parseable reply (mirrors
    /// [`super::SemanticOutcome::call`]'s own condition). Phase 58, map line
    /// 2030: named on the applied line so a reduced result's header says
    /// which reducer produced it — model-backed and local reducers alike,
    /// symmetrically.
    pub reducer: Option<String>,
}

impl SemanticProvenance {
    fn render(&self) -> String {
        if self.applied {
            let by = self
                .reducer
                .as_deref()
                .map(|reducer| format!(" by {reducer}"))
                .unwrap_or_default();
            format!(
                "[glasshouse context firewall: semantic reduction{by} kept {}/{} candidates]\n",
                self.kept, self.considered
            )
        } else {
            format!(
                "[glasshouse context firewall: semantic reduction bypassed ({})]\n",
                self.reason.as_deref().unwrap_or("unknown")
            )
        }
    }
}

/// What the header states about one reduction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub original_tokens: u64,
    pub forwarded_tokens: u64,
    pub retained_candidates: usize,
    pub total_candidates: usize,
    pub raw_ref: String,
    /// Present only when the semantic stage was attempted — see
    /// [`SemanticProvenance`].
    pub semantic: Option<SemanticProvenance>,
}

impl Provenance {
    /// Render the header. Deliberately plain text, not a format reduction
    /// itself needs to parse back — the raw store round-trip (map line
    /// 1984/1985) never reads this, only `original_bytes`.
    pub fn render(&self) -> String {
        let mut out = format!(
            "[glasshouse context firewall: reduced ~{orig} tokens to ~{fwd}; kept {retained}/{total} candidates; raw: {raw}]\n",
            orig = self.original_tokens,
            fwd = self.forwarded_tokens,
            retained = self.retained_candidates,
            total = self.total_candidates,
            raw = self.raw_ref,
        );
        if let Some(semantic) = &self.semantic {
            out.push_str(&semantic.render());
        }
        out
    }

    /// Prefix this header onto `body`.
    pub fn prepend_to(&self, body: &str) -> String {
        let mut out = self.render();
        out.push_str(body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_carries_every_stated_field() {
        let provenance = Provenance {
            original_tokens: 9000,
            forwarded_tokens: 1200,
            retained_candidates: 5,
            total_candidates: 400,
            raw_ref: "gh-tool://abc123".to_string(),
            semantic: None,
        };
        let rendered = provenance.render();
        assert!(rendered.contains("9000"));
        assert!(rendered.contains("1200"));
        assert!(rendered.contains("5/400"));
        assert!(rendered.contains("gh-tool://abc123"));
    }

    #[test]
    fn prepend_puts_the_header_before_the_body_untouched() {
        let provenance = Provenance {
            original_tokens: 10,
            forwarded_tokens: 5,
            retained_candidates: 1,
            total_candidates: 2,
            raw_ref: "gh-tool://x".to_string(),
            semantic: None,
        };
        let body = "the reduced content\n";
        let out = provenance.prepend_to(body);
        assert!(out.ends_with(body));
        assert!(out.starts_with("[glasshouse context firewall:"));
    }

    #[test]
    fn no_semantic_line_appears_when_the_stage_was_never_attempted() {
        let provenance = Provenance {
            original_tokens: 10,
            forwarded_tokens: 5,
            retained_candidates: 1,
            total_candidates: 2,
            raw_ref: "gh-tool://x".to_string(),
            semantic: None,
        };
        assert!(!provenance.render().contains("semantic"));
    }

    /// The applied half: the header states how many of the candidates the
    /// semantic stage was given actually survived.
    #[test]
    fn an_applied_semantic_line_states_kept_and_considered() {
        let provenance = Provenance {
            original_tokens: 9000,
            forwarded_tokens: 400,
            retained_candidates: 5,
            total_candidates: 400,
            raw_ref: "gh-tool://abc123".to_string(),
            semantic: Some(SemanticProvenance {
                applied: true,
                kept: 3,
                considered: 5,
                reason: None,
                reducer: None,
            }),
        };
        let rendered = provenance.render();
        assert!(rendered.contains("semantic reduction kept 3/5"));
    }

    /// Phase 58, map line 2030: named on the applied line so a header can
    /// say which reducer produced a reduction — the local reducer's own
    /// shape (`"local:<name> <tool_version>"`), and symmetrically the
    /// model-backed reducer's own `"<provider> <model>"`.
    #[test]
    fn an_applied_semantic_line_names_the_reducer_when_known() {
        let provenance = Provenance {
            original_tokens: 9000,
            forwarded_tokens: 400,
            retained_candidates: 5,
            total_candidates: 400,
            raw_ref: "gh-tool://abc123".to_string(),
            semantic: Some(SemanticProvenance {
                applied: true,
                kept: 3,
                considered: 5,
                reason: None,
                reducer: Some("local:headroom 0.9.3".to_string()),
            }),
        };
        let rendered = provenance.render();
        assert!(rendered.contains("semantic reduction by local:headroom 0.9.3 kept 3/5"));
    }

    /// The bypassed half: the header names the reason, never leaves a
    /// silent gap where the reader might assume nothing was attempted.
    #[test]
    fn a_bypassed_semantic_line_states_the_reason() {
        let provenance = Provenance {
            original_tokens: 9000,
            forwarded_tokens: 400,
            retained_candidates: 5,
            total_candidates: 400,
            raw_ref: "gh-tool://abc123".to_string(),
            semantic: Some(SemanticProvenance {
                applied: false,
                kept: 0,
                considered: 5,
                reason: Some("reducer-timed-out".to_string()),
                reducer: None,
            }),
        };
        let rendered = provenance.render();
        assert!(rendered.contains("semantic reduction bypassed (reducer-timed-out)"));
    }
}
