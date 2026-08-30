use serde::{Deserialize, Serialize};
use whale_protocol::TokenUsage;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PriceRate {
    pub provider: Option<String>,
    pub models: Vec<String>,
    pub reasoning_effort: Option<String>,
    pub priority: i64,
    pub input_usd_micros_per_million: i64,
    pub cache_read_usd_micros_per_million: i64,
    pub cache_write_usd_micros_per_million: i64,
    pub output_usd_micros_per_million: i64,
    pub reasoning_usd_micros_per_million: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PriceCatalog {
    pub version: String,
    pub rates: Vec<PriceRate>,
}

impl PriceCatalog {
    pub fn estimate(
        &self,
        provider: &str,
        model: &str,
        alias: &str,
        reasoning_effort: Option<&str>,
        tokens: &TokenUsage,
    ) -> Option<i64> {
        let rate = self.best_rate(provider, model, alias, reasoning_effort)?;

        let uncached_input = tokens
            .input_tokens
            .saturating_sub(tokens.cache_read_tokens)
            .saturating_sub(tokens.cache_write_tokens)
            .max(0);
        let non_reasoning_output = tokens
            .output_tokens
            .saturating_sub(tokens.reasoning_tokens)
            .max(0);
        let numerator = (uncached_input as i128) * (rate.input_usd_micros_per_million as i128)
            + (tokens.cache_read_tokens as i128) * (rate.cache_read_usd_micros_per_million as i128)
            + (tokens.cache_write_tokens as i128)
                * (rate.cache_write_usd_micros_per_million as i128)
            + (non_reasoning_output as i128) * (rate.output_usd_micros_per_million as i128)
            + (tokens.reasoning_tokens as i128) * (rate.reasoning_usd_micros_per_million as i128);
        let rounded = (numerator + 500_000) / 1_000_000;
        i64::try_from(rounded).ok()
    }

    pub fn has_rate(&self, provider: &str, model: &str) -> bool {
        self.best_rate(provider, model, model, None).is_some()
            || self.rates.iter().any(|rate| {
                provider_matches(rate.provider.as_deref(), provider)
                    && model_matches(rate, model, model)
            })
    }

    fn best_rate(
        &self,
        provider: &str,
        model: &str,
        alias: &str,
        reasoning_effort: Option<&str>,
    ) -> Option<&PriceRate> {
        self.rates
            .iter()
            .filter(|rate| provider_matches(rate.provider.as_deref(), provider))
            .filter(|rate| model_matches(rate, model, alias))
            .filter(|rate| {
                rate.reasoning_effort.is_none()
                    || option_eq(rate.reasoning_effort.as_deref(), reasoning_effort)
            })
            .max_by_key(|rate| (rate.priority, i64::from(rate.reasoning_effort.is_some())))
    }
}

fn provider_matches(expected: Option<&str>, actual: &str) -> bool {
    expected
        .map(|expected| expected.eq_ignore_ascii_case(actual))
        .unwrap_or(true)
}

fn model_matches(rate: &PriceRate, model: &str, alias: &str) -> bool {
    rate.models.iter().any(|candidate| {
        candidate.eq_ignore_ascii_case(model) || candidate.eq_ignore_ascii_case(alias)
    })
}

fn option_eq(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate() -> PriceRate {
        PriceRate {
            provider: Some("codex".into()),
            models: vec!["gpt-test".into(), "test-alias".into()],
            reasoning_effort: Some("xhigh".into()),
            priority: 10,
            input_usd_micros_per_million: 1_000_000,
            cache_read_usd_micros_per_million: 100_000,
            cache_write_usd_micros_per_million: 500_000,
            output_usd_micros_per_million: 2_000_000,
            reasoning_usd_micros_per_million: 3_000_000,
        }
    }

    #[test]
    fn uses_provider_alias_and_reasoning_specific_rate() {
        let catalog = PriceCatalog {
            version: "test".into(),
            rates: vec![rate()],
        };
        let tokens = TokenUsage {
            input_tokens: 1_000_000,
            cache_read_tokens: 200_000,
            cache_write_tokens: 100_000,
            output_tokens: 500_000,
            reasoning_tokens: 100_000,
            total_tokens: 1_500_000,
            ..TokenUsage::default()
        };
        assert_eq!(
            catalog.estimate("codex", "other-id", "test-alias", Some("xhigh"), &tokens),
            Some(1_870_000)
        );
        assert_eq!(
            catalog.estimate("other", "gpt-test", "", Some("xhigh"), &tokens),
            None
        );
    }

    #[test]
    fn higher_priority_matching_rate_wins() {
        let mut fallback = rate();
        fallback.provider = None;
        fallback.reasoning_effort = None;
        fallback.priority = 1;
        fallback.input_usd_micros_per_million = 10;
        let catalog = PriceCatalog {
            version: "test".into(),
            rates: vec![fallback, rate()],
        };
        let tokens = TokenUsage {
            input_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        assert_eq!(
            catalog.estimate("codex", "gpt-test", "", Some("xhigh"), &tokens),
            Some(1_000_000)
        );
    }

    #[test]
    fn unpriced_model_is_unknown() {
        assert_eq!(
            PriceCatalog::default().estimate(
                "missing",
                "missing",
                "",
                None,
                &TokenUsage::default()
            ),
            None
        );
    }
}
