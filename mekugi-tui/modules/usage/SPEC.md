# Provider usage accounting

## Requirements

- Track cumulative per-thread provider-authoritative token usage, reference API cost estimates, and
  measured output throughput alongside execution without adding usage text to model context or
  writing Markdown usage files.
- Keep missing or partial usage/pricing explicitly labeled (never inferred as zero or confused
  with provider billing quotes).

## Current approach (replaceable)

- The client sums individual native provider completions and keeps the
  latest valid output measurement per thread. Forked threads start their
  own counters; inherited context is not spending.
- Reference prices apply per OpenAI response, including returned service tier and applicable
  long-context and cache rates. Custom providers and unknown prices do not acquire an estimate.
- The native runtime retains these values in session history.
  Reopened records retain their reported totals and are marked partial.
- Composer captions and roster rows share the output throughput measurement. Context occupancy
  remains separate from cumulative provider usage.
