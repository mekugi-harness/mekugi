# Transport diagnostics

## Requirements

Inherited from [product requirements](../../../docs/product.md#user-requirements): capture content-safe
transport, timing, cache-prefix, and provider-stream diagnostics alongside execution without
altering routed payloads, adding network hops, exposing credentials/prompts/raw errors in sanitized
artifacts, or affecting request outcomes.

## Live measurements

`/session` counts native completed responses and terminal turn errors in this
launch. It retains the latest 64 rows without resetting totals. Usage comes
from the provider. Throughput pairs observed output with measured provider
time. Native rows do not include individual retries, wire sizes or cache
prefix comparisons.

## Private and retained evidence

`--capture-output PATH` appends private JSONL with thread, turn, model, status,
and any measured time and reported usage. The log excludes prompts, headers,
provider response IDs and raw errors. Live measurements need no capture file.

Inspect and replay can still read historical gateway captures. A native
capture has no loopback boundary. Removing the module disables these
diagnostics without changing inference.
