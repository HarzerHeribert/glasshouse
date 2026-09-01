//! Token-count estimation for the ladder's threshold check.
//!
//! No existing estimator fits: the packet asked to reuse one from
//! `provider/`/`gateway/` first, and neither module has a text-to-token
//! estimator — both only handle provider-reported counts and quota
//! percentages. A `chars / 4` heuristic is the documented fallback the
//! packet accepted, so that is what this is: a rough English/code average,
//! never a claim about any real tokenizer.

/// Estimate how many tokens `text` is worth, at roughly four characters per
/// token. Rounds up so a non-empty string is never estimated at zero.
pub fn estimate_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_zero_tokens() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn a_short_non_empty_string_never_rounds_to_zero() {
        assert_eq!(estimate_tokens("hi"), 1);
    }

    #[test]
    fn four_chars_per_token_rounds_up() {
        assert_eq!(estimate_tokens("12345"), 2);
        assert_eq!(estimate_tokens("1234"), 1);
    }
}
