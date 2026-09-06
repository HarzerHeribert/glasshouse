//! Local measurements for provider requests. These values are presentation
//! state only: they are never rendered into a model message or rollout.

use crate::contract::ServedBy;
use crate::wire::Usage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestMeasurement {
    pub cell: usize,
    pub model: String,
    pub elapsed_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub served: ServedBy,
}

impl RequestMeasurement {
    /// `served` must be correlated to this exact request. Project-wide routing
    /// observations cannot safely be attributed here during concurrent work.
    pub fn from_response(
        cell: usize,
        model: String,
        elapsed_ms: u64,
        served: ServedBy,
        usage: Option<&Usage>,
    ) -> Self {
        let input_tokens = served
            .input_tokens
            .or_else(|| usage.map(|row| row.input_tokens));
        let output_tokens = served
            .output_tokens
            .or_else(|| usage.map(|row| row.output_tokens));
        Self {
            cell,
            model,
            elapsed_ms,
            input_tokens,
            output_tokens,
            cached_input_tokens: served.cached_input_tokens,
            served,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlated_gateway_fields_take_precedence_individually() {
        let served = ServedBy {
            input_tokens: Some(30),
            output_tokens: None,
            cached_input_tokens: Some(20),
            ..ServedBy::default()
        };
        let usage = Usage {
            input_tokens: 300,
            output_tokens: 40,
        };
        let measured = RequestMeasurement::from_response(
            2,
            "requested".into(),
            17,
            served.clone(),
            Some(&usage),
        );
        assert_eq!(measured.input_tokens, Some(30));
        assert_eq!(measured.output_tokens, Some(40));
        assert_eq!(measured.cached_input_tokens, Some(20));
        assert_eq!(measured.served, served);
    }

    #[test]
    fn response_usage_is_used_only_when_gateway_tokens_are_absent() {
        let served = ServedBy {
            provider: Some("provider".into()),
            ..ServedBy::default()
        };
        let usage = Usage {
            input_tokens: 12,
            output_tokens: 4,
        };
        let measured =
            RequestMeasurement::from_response(1, "model".into(), 9, served, Some(&usage));
        assert_eq!(measured.input_tokens, Some(12));
        assert_eq!(measured.output_tokens, Some(4));
        assert_eq!(measured.cached_input_tokens, None);
    }

    #[test]
    fn absent_usage_stays_unknown() {
        let measured =
            RequestMeasurement::from_response(1, "model".into(), 9, ServedBy::default(), None);
        assert_eq!(measured.input_tokens, None);
        assert_eq!(measured.output_tokens, None);
        assert_eq!(measured.cached_input_tokens, None);
    }
}
