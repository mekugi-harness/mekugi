# Responses transport integration

## Requirements

- Codex remains root execution authority with one stock JavaScript execution interface (`exec` with
  nested stock tools). Top-level `apply_patch`/`exec_command` catalogs and direct execution mode are
  unsupported.
- Preserve stock WebSocket/HTTP fallback, prewarm, streaming, steering, interruption, batching, and
  existing features while improving [product outcomes](../product.md#user-requirements).

## Boundary contract

- **Endpoints**: The native model client connects directly to the selected
  provider for Responses, model catalogs and supported auxiliary requests.
  See [provider routing](provider-routing.md) for endpoint and auth rules.
- **Prewarm**: A non-generating WebSocket handshake prepares a connection;
  it does not complete a turn or contribute provider usage.
- **Projection boundary**: projection never mutates original tool arguments, host history, or
  retained evidence. Native [output deduplication](../../mekugi-tui/backends/codex/SPEC.md#output-deduplication)
  runs before transport; growing input preserves projected prefixes.
- **Continuations**: A WebSocket continuation extends the projected prefix already
  sent. Context reset rebuilds the selected model input without shortening
  retained review outputs.
- **Failures**: Cancellation, provider errors and usage belong to the native
  request and its thread. A failed send or disconnect must not silently
  replay a request or steer. Detaching a renderer does not cancel inference.
- **Deadlines**: Codex owns provider timeouts. Custom providers can set
  `stream_idle_timeout_ms` and `websocket_connect_timeout_ms` in their
  provider configuration.
