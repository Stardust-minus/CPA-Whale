use whale_protocol::{TokenUsage, UsageTotals};

pub fn add_totals(target: &mut UsageTotals, value: &UsageTotals) {
    let target_was_empty = target.requests == 0;
    target.requests = target.requests.saturating_add(value.requests);
    target.successful_requests = target
        .successful_requests
        .saturating_add(value.successful_requests);
    target.failed_requests = target.failed_requests.saturating_add(value.failed_requests);
    add_tokens(&mut target.tokens, &value.tokens);
    target.estimated_usd_micros = if target_was_empty {
        value.estimated_usd_micros
    } else {
        match (target.estimated_usd_micros, value.estimated_usd_micros) {
            (Some(left), Some(right)) => Some(left.saturating_add(right)),
            _ => None,
        }
    };
}

pub fn subtract_totals(current: &UsageTotals, baseline: &UsageTotals) -> UsageTotals {
    UsageTotals {
        requests: nonnegative_sub(current.requests, baseline.requests),
        successful_requests: nonnegative_sub(
            current.successful_requests,
            baseline.successful_requests,
        ),
        failed_requests: nonnegative_sub(current.failed_requests, baseline.failed_requests),
        tokens: subtract_tokens(&current.tokens, &baseline.tokens),
        estimated_usd_micros: match (current.estimated_usd_micros, baseline.estimated_usd_micros) {
            (Some(left), Some(right)) => Some(nonnegative_sub(left, right)),
            _ => None,
        },
    }
}

fn add_tokens(target: &mut TokenUsage, value: &TokenUsage) {
    target.input_tokens = target.input_tokens.saturating_add(value.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(value.output_tokens);
    target.reasoning_tokens = target
        .reasoning_tokens
        .saturating_add(value.reasoning_tokens);
    target.cached_tokens = target.cached_tokens.saturating_add(value.cached_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(value.cache_read_tokens);
    target.cache_write_tokens = target
        .cache_write_tokens
        .saturating_add(value.cache_write_tokens);
    target.total_tokens = target.total_tokens.saturating_add(value.total_tokens);
}

fn subtract_tokens(current: &TokenUsage, baseline: &TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: nonnegative_sub(current.input_tokens, baseline.input_tokens),
        output_tokens: nonnegative_sub(current.output_tokens, baseline.output_tokens),
        reasoning_tokens: nonnegative_sub(current.reasoning_tokens, baseline.reasoning_tokens),
        cached_tokens: nonnegative_sub(current.cached_tokens, baseline.cached_tokens),
        cache_read_tokens: nonnegative_sub(current.cache_read_tokens, baseline.cache_read_tokens),
        cache_write_tokens: nonnegative_sub(
            current.cache_write_tokens,
            baseline.cache_write_tokens,
        ),
        total_tokens: nonnegative_sub(current.total_tokens, baseline.total_tokens),
    }
}

fn nonnegative_sub(current: i64, baseline: i64) -> i64 {
    current.saturating_sub(baseline).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtracts_without_underflow() {
        let current = UsageTotals {
            requests: 5,
            tokens: TokenUsage {
                total_tokens: 10,
                ..TokenUsage::default()
            },
            ..UsageTotals::default()
        };
        let baseline = UsageTotals {
            requests: 8,
            tokens: TokenUsage {
                total_tokens: 20,
                ..TokenUsage::default()
            },
            ..UsageTotals::default()
        };
        let delta = subtract_totals(&current, &baseline);
        assert_eq!(delta.requests, 0);
        assert_eq!(delta.tokens.total_tokens, 0);
    }
}
