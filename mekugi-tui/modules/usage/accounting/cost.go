package accounting

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
)

// Cost contains already-priced estimates. Reported holds native estimates that
// do not provide a category breakdown. Reasoning is already included in output;
// cached input is already included in input.
type Cost struct {
	UncachedInput float64
	CachedInput   float64
	Output        float64
	Reported      float64
	Known         bool
}

type Report struct {
	inference.Usage
	Cost                     float64
	CostKnown                bool
	MissingUsage, Roundtrips uint64
	PriorUnknown             bool
	OutputThroughput         inference.OutputThroughput
}

// Source: session-usage/scripts/session_usage.py:625:655 select_tier and cost_for_request.
// OpenAI rates mirror FALLBACK_USD_PER_MILLION in the same script.
// These are reference list API prices in USD per million tokens, not subscription
// charges or live billing quotes. Other providers need explicitly supplied rates.
// GPT-6.1 Sol source (2026-09-30): https://developers.openai.com/api/docs/pricing
// GPT-6.1 Terra source (2026-10-27): https://developers.openai.com/api/docs/models/gpt-6.1-terra
// GPT-6.2 Astra and Sol source (2026-11-18): https://developers.openai.com/api/docs/pricing

// Fast and priority are aliases. These are model-specific rates, not a blanket
// multiplier: GPT-5.5 has a different premium and no published long-context rate.
// Source (2026-09-12): https://developers.openai.com/api/docs/pricing#text-tokens
