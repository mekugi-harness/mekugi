//! Per-thread reference cost, output throughput, work duration, and roundtrip accounting.
//!
//! Each completed model response (`EventMsg::RawResponseCompleted`) is priced individually
//! using its own model, service tier, and single-response input token bracket so later model
//! or tier switches never reprice earlier turns. Output throughput divides only measured
//! response output tokens by measured response request durations so resumed threads never
//! inflate throughput with historical tokens.

use std::time::Instant;

use codex_app_server_protocol::ThreadTokenUsage;
use codex_protocol::protocol::RawResponseCompletedEvent;
use codex_protocol::protocol::TokenUsage;

const LONG_CONTEXT_THRESHOLD: i64 = 272_000;

#[derive(Debug, Clone, Copy)]
struct TokenPrice {
    input: f64,
    cached_input: f64,
    output: f64,
    long_input: f64,
    long_cached_input: f64,
    long_output: f64,
}

impl TokenPrice {
    const fn new(
        input: f64,
        cached_input: f64,
        output: f64,
        long_input: f64,
        long_cached_input: f64,
        long_output: f64,
    ) -> Self {
        Self {
            input,
            cached_input,
            output,
            long_input,
            long_cached_input,
            long_output,
        }
    }

    fn has_long_context(self, input_tokens: i64) -> bool {
        self.long_input > 0.0 && input_tokens > LONG_CONTEXT_THRESHOLD
    }
}

/// Reference list API prices in USD per million tokens (standard tier).
fn standard_reference_price(model: &str) -> Option<TokenPrice> {
    match model {
        "gpt-6.2-astra" => Some(TokenPrice::new(8.0, 0.4, 40.0, 16.0, 0.8, 60.0)),
        "gpt-6.2-sol" => Some(TokenPrice::new(2.0, 0.1, 10.0, 4.0, 0.2, 15.0)),
        "gpt-6-astra" | "gpt-6-astra-pro" => {
            Some(TokenPrice::new(10.0, 1.0, 50.0, 20.0, 2.0, 75.0))
        }
        "gpt-6.1-sol" => Some(TokenPrice::new(2.0, 0.1, 10.0, 4.0, 0.2, 15.0)),
        "gpt-6.1-terra" => Some(TokenPrice::new(1.0, 0.05, 5.0, 2.0, 0.1, 7.5)),
        "gpt-6-sol" => Some(TokenPrice::new(2.0, 0.2, 10.0, 4.0, 0.4, 15.0)),
        "gpt-6-luna" => Some(TokenPrice::new(0.1, 0.01, 0.5, 0.2, 0.02, 0.75)),
        "gpt-5.5" => Some(TokenPrice::new(5.0, 0.5, 30.0, 10.0, 1.0, 45.0)),
        "gpt-5.6-sol" => Some(TokenPrice::new(4.0, 0.4, 20.0, 8.0, 0.8, 30.0)),
        "gpt-5.6-terra" => Some(TokenPrice::new(2.0, 0.2, 12.0, 4.0, 0.4, 18.0)),
        "gpt-5.6-luna" => Some(TokenPrice::new(0.2, 0.02, 1.2, 0.4, 0.04, 1.8)),
        "gpt-5.4" => Some(TokenPrice::new(2.5, 0.25, 15.0, 5.0, 0.5, 22.5)),
        "gpt-5.4-mini" => Some(TokenPrice::new(0.75, 0.075, 4.5, 0.0, 0.0, 0.0)),
        "gpt-5.4-nano" => Some(TokenPrice::new(0.2, 0.02, 1.25, 0.0, 0.0, 0.0)),
        _ => None,
    }
}

/// Reference list API prices in USD per million tokens (fast / priority tier).
fn fast_reference_price(model: &str) -> Option<TokenPrice> {
    match model {
        "gpt-6.2-astra" => Some(TokenPrice::new(16.0, 0.8, 80.0, 32.0, 1.6, 120.0)),
        "gpt-6.2-sol" => Some(TokenPrice::new(4.0, 0.2, 20.0, 8.0, 0.4, 30.0)),
        "gpt-6-astra" => Some(TokenPrice::new(20.0, 2.0, 100.0, 40.0, 4.0, 150.0)),
        "gpt-6.1-sol" => Some(TokenPrice::new(4.0, 0.2, 20.0, 8.0, 0.4, 30.0)),
        "gpt-6.1-terra" => Some(TokenPrice::new(2.0, 0.1, 10.0, 4.0, 0.2, 15.0)),
        "gpt-6-sol" => Some(TokenPrice::new(4.0, 0.4, 20.0, 8.0, 0.8, 30.0)),
        "gpt-6-luna" => Some(TokenPrice::new(0.2, 0.02, 1.0, 0.4, 0.04, 1.5)),
        "gpt-5.6-sol" => Some(TokenPrice::new(8.0, 0.8, 40.0, 16.0, 1.6, 60.0)),
        "gpt-5.6-terra" => Some(TokenPrice::new(4.0, 0.4, 24.0, 8.0, 0.8, 36.0)),
        "gpt-5.6-luna" => Some(TokenPrice::new(0.4, 0.04, 2.4, 0.8, 0.08, 3.6)),
        "gpt-5.5" => Some(TokenPrice::new(12.5, 1.25, 75.0, 0.0, 0.0, 0.0)),
        "gpt-5.4" => Some(TokenPrice::new(5.0, 0.5, 30.0, 0.0, 0.0, 0.0)),
        "gpt-5.4-mini" => Some(TokenPrice::new(1.5, 0.15, 9.0, 0.0, 0.0, 0.0)),
        _ => None,
    }
}

fn supports_cache_write_premium(model: &str) -> bool {
    matches!(
        model,
        "gpt-6.2-astra"
            | "gpt-6.2-sol"
            | "gpt-6-astra"
            | "gpt-6.1-sol"
            | "gpt-6.1-terra"
            | "gpt-6-sol"
            | "gpt-6-luna"
            | "gpt-5.6-sol"
            | "gpt-5.6-terra"
            | "gpt-5.6-luna"
    )
}

fn normalize_model(model: &str) -> String {
    let trimmed = model.trim().to_ascii_lowercase();
    trimmed
        .strip_prefix("openai/")
        .unwrap_or(&trimmed)
        .to_string()
}

/// Estimates USD cost for one completed provider response using its own model, tier,
/// and per-response input bracket.
pub(crate) fn estimate_response_cost_usd(
    model: &str,
    service_tier: Option<&str>,
    usage: &TokenUsage,
) -> Option<f64> {
    if usage.input_tokens < 0
        || usage.cached_input_tokens < 0
        || usage.cache_write_input_tokens < 0
        || usage.output_tokens < 0
        || usage.cached_input_tokens > usage.input_tokens
    {
        return None;
    }
    let normalized = normalize_model(model);
    let standard = standard_reference_price(&normalized);
    let tier = service_tier.unwrap_or("").trim().to_ascii_lowercase();
    let price = match tier.as_str() {
        "" | "default" | "standard" => standard?,
        "fast" | "priority" => {
            let fast = fast_reference_price(&normalized)?;
            if standard.is_some_and(|std_price| std_price.has_long_context(usage.input_tokens))
                && fast.long_input == 0.0
            {
                return None;
            }
            fast
        }
        _ => return None,
    };

    let uncached = usage.input_tokens - usage.cached_input_tokens;
    if usage.cache_write_input_tokens > uncached {
        return None;
    }
    let long_context = price.has_long_context(usage.input_tokens);
    let (input_rate, cached_rate, output_rate) = if long_context {
        (price.long_input, price.long_cached_input, price.long_output)
    } else {
        (price.input, price.cached_input, price.output)
    };
    if usage.cache_write_input_tokens > 0 && !supports_cache_write_premium(&normalized) {
        return None;
    }
    let cache_write_premium =
        (usage.cache_write_input_tokens as f64) * input_rate * 0.25 / 1_000_000.0;
    Some(
        (uncached as f64) * input_rate / 1_000_000.0
            + cache_write_premium
            + (usage.cached_input_tokens as f64) * cached_rate / 1_000_000.0
            + (usage.output_tokens as f64) * output_rate / 1_000_000.0,
    )
}

#[derive(Debug, Default)]
pub(crate) struct ThreadUsageMetrics {
    estimated_cost_usd: f64,
    priced_responses: u64,
    unpriced_responses: u64,
    prior_unknown: bool,
    roundtrips: u64,
    measured_output_tokens: u64,
    measured_duration_ms: u64,
    completed_work_ms: u64,
    active_turn_started: Option<Instant>,
}

impl ThreadUsageMetrics {
    pub(crate) fn mark_prior_unknown(&mut self) {
        self.prior_unknown = true;
    }

    pub(crate) fn has_observed_responses(&self) -> bool {
        self.roundtrips > 0
    }

    pub(crate) fn observe_turn_started(&mut self) {
        self.active_turn_started = Some(Instant::now());
    }

    pub(crate) fn observe_turn_finished(&mut self, duration_ms: Option<i64>) {
        let elapsed_ms = duration_ms
            .and_then(|ms| u64::try_from(ms).ok())
            .or_else(|| {
                self.active_turn_started
                    .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
            })
            .unwrap_or(0);
        self.active_turn_started = None;
        self.completed_work_ms = self.completed_work_ms.saturating_add(elapsed_ms);
    }

    pub(crate) fn observe_raw_response(&mut self, event: &RawResponseCompletedEvent) {
        self.roundtrips = self.roundtrips.saturating_add(1);
        let Some(usage) = event.token_usage.as_ref() else {
            self.unpriced_responses = self.unpriced_responses.saturating_add(1);
            return;
        };

        if let Some(duration_ms) = event.request_duration_ms
            && duration_ms > 0
            && usage.output_tokens >= 0
        {
            self.measured_output_tokens = self
                .measured_output_tokens
                .saturating_add(usage.output_tokens as u64);
            self.measured_duration_ms =
                self.measured_duration_ms.saturating_add(duration_ms as u64);
        }

        let reported_usd = event
            .usage_metadata
            .as_ref()
            .and_then(|meta| meta.amount.as_deref())
            .and_then(|amount| amount.trim().parse::<f64>().ok())
            .filter(|amount| amount.is_finite() && *amount >= 0.0);

        let response_cost = reported_usd.or_else(|| {
            if !event.openai_endpoint {
                return None;
            }
            let model = event.model.as_deref()?;
            estimate_response_cost_usd(model, event.service_tier.as_deref(), usage)
        });

        if let Some(cost) = response_cost {
            self.estimated_cost_usd += cost;
            self.priced_responses = self.priced_responses.saturating_add(1);
        } else {
            self.unpriced_responses = self.unpriced_responses.saturating_add(1);
        }
    }

    pub(crate) fn apply_to(&self, usage: &mut ThreadTokenUsage) {
        if self.priced_responses > 0 {
            usage.estimated_cost_usd = Some(self.estimated_cost_usd);
            usage.cost_partial = self.prior_unknown || self.unpriced_responses > 0;
        }
        if self.measured_duration_ms > 0 && self.measured_output_tokens > 0 {
            usage.output_tokens_per_second = Some(
                (self.measured_output_tokens as f64) * 1000.0 / (self.measured_duration_ms as f64),
            );
        }
        let live_work_ms = self
            .active_turn_started
            .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(0))
            .unwrap_or(0);
        let work_ms = self.completed_work_ms.saturating_add(live_work_ms);
        if work_ms > 0 {
            usage.work_duration_ms = i64::try_from(work_ms).ok();
        }
        if self.roundtrips > 0 {
            usage.roundtrips = i64::try_from(self.roundtrips).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::TokenUsageBreakdown;
    use pretty_assertions::assert_eq;

    fn sample_usage(input: i64, cached: i64, output: i64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            cached_input_tokens: cached,
            cache_write_input_tokens: 0,
            output_tokens: output,
            reasoning_output_tokens: 0,
            total_tokens: input + output,
            codex_rollout_budget_units: None,
        }
    }

    fn empty_thread_usage() -> ThreadTokenUsage {
        let zero = TokenUsageBreakdown {
            total_tokens: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            cache_write_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
        };
        ThreadTokenUsage {
            total: zero.clone(),
            last: zero,
            model_context_window: None,
            estimated_cost_usd: None,
            cost_partial: false,
            output_tokens_per_second: None,
            work_duration_ms: None,
            roundtrips: None,
        }
    }

    #[test]
    fn switching_model_and_tier_does_not_reprice_earlier_responses() {
        let mut metrics = ThreadUsageMetrics::default();
        // Response 1: gpt-6.2-sol standard, 1M uncached input ($2) + 1M output ($10) = $12
        metrics.observe_raw_response(&RawResponseCompletedEvent {
            response_id: "r1".to_string(),
            token_usage: Some(sample_usage(100_000, 0, 100_000)),
            usage_metadata: None,
            model: Some("gpt-6.2-sol".to_string()),
            service_tier: None,
            openai_endpoint: true,
            request_duration_ms: Some(2_000),
        });
        // Response 2: switched to gpt-6.2-astra priority, 100k uncached input ($1.60) + 100k output ($8.00) = $9.60
        metrics.observe_raw_response(&RawResponseCompletedEvent {
            response_id: "r2".to_string(),
            token_usage: Some(sample_usage(100_000, 0, 100_000)),
            usage_metadata: None,
            model: Some("gpt-6.2-astra".to_string()),
            service_tier: Some("priority".to_string()),
            openai_endpoint: true,
            request_duration_ms: Some(2_000),
        });

        let mut out = empty_thread_usage();
        metrics.apply_to(&mut out);
        let cost = out.estimated_cost_usd.expect("cost should be known");
        // $1.20 + $9.60 = $10.80 (would be $19.20 if cumulative tokens were repriced at astra priority)
        assert!((cost - 10.80).abs() < 1e-9, "got cost {cost}");
        assert_eq!(out.cost_partial, false);
        assert_eq!(out.roundtrips, Some(2));
        // 200_000 output tokens over 4_000 ms = 50_000 tok/s
        assert!((out.output_tokens_per_second.unwrap() - 50_000.0).abs() < 1e-6);
    }

    #[test]
    fn long_context_bracket_applies_per_response_not_cumulatively() {
        let mut metrics = ThreadUsageMetrics::default();
        // Three 150k-input responses (cumulative 450k > 272k, but each <= 272k -> standard rate)
        for idx in 0..3 {
            metrics.observe_raw_response(&RawResponseCompletedEvent {
                response_id: format!("r{idx}"),
                token_usage: Some(sample_usage(150_000, 0, 0)),
                usage_metadata: None,
                model: Some("gpt-6.2-sol".to_string()),
                service_tier: None,
                openai_endpoint: true,
                request_duration_ms: Some(1_000),
            });
        }
        let mut out = empty_thread_usage();
        metrics.apply_to(&mut out);
        // 450k * $2/M = $0.90 (not long-context $4/M = $1.80)
        assert!((out.estimated_cost_usd.unwrap() - 0.90).abs() < 1e-9);
    }

    #[test]
    fn resumed_thread_marks_cost_partial_and_does_not_inflate_throughput() {
        let mut metrics = ThreadUsageMetrics::default();
        metrics.mark_prior_unknown();

        // Resumed thread had 50k prior output tokens; new response produces 200 tokens in 2000ms.
        metrics.observe_raw_response(&RawResponseCompletedEvent {
            response_id: "r-new".to_string(),
            token_usage: Some(sample_usage(1_000, 0, 200)),
            usage_metadata: None,
            model: Some("gpt-6.2-sol".to_string()),
            service_tier: None,
            openai_endpoint: true,
            request_duration_ms: Some(2_000),
        });

        let mut out = empty_thread_usage();
        out.total.output_tokens = 50_200;
        metrics.apply_to(&mut out);

        assert_eq!(out.cost_partial, true);
        // Must be 200 / 2.0s = 100 tok/s, never 50_200 / 2.0s.
        assert!((out.output_tokens_per_second.unwrap() - 100.0).abs() < 1e-6);
    }
}
