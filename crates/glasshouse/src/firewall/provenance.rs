//! The provenance header — map line 1986. One compact block prefixed to a
//! reduced result; nothing is ever added to a passthrough result.

/// What the header states about one reduction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub original_tokens: u64,
    pub forwarded_tokens: u64,
    pub retained_candidates: usize,
    pub total_candidates: usize,
    pub raw_ref: String,
}

impl Provenance {
    /// Render the header. Deliberately plain text, not a format reduction
    /// itself needs to parse back — the raw store round-trip (map line
    /// 1984/1985) never reads this, only `original_bytes`.
    pub fn render(&self) -> String {
        format!(
            "[glasshouse context firewall: reduced ~{orig} tokens to ~{fwd}; kept {retained}/{total} candidates; raw: {raw}]\n",
            orig = self.original_tokens,
            fwd = self.forwarded_tokens,
            retained = self.retained_candidates,
            total = self.total_candidates,
            raw = self.raw_ref,
        )
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
        };
        let body = "the reduced content\n";
        let out = provenance.prepend_to(body);
        assert!(out.ends_with(body));
        assert!(out.starts_with("[glasshouse context firewall:"));
    }
}
