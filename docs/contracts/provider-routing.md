# Provider routing and thread continuity

## Requirements

- Support native OpenAI (`mekugi codex`) and explicitly configured custom Responses providers
  (`mekugi -p NAME`) while preserving Codex execution, tools, permissions, batching, continuation,
  and per-thread usage.
- Native OpenAI must not depend on a proxy service; custom providers must use the profile's actual
  provider, model, endpoint, and auth settings rather than model-name prefix routing.

## Boundary contract

- **Pinning and isolation**: The app server binds each family to its admitted
  provider and endpoint. Requests go directly to that endpoint, preserving
  path prefixes, queries and configured HTTP/SSE or WebSocket transport.
  Custom routes do not redirect credentials, receive OpenAI-only headers,
  or fall back to OpenAI on failure.
- **Authentication**: Codex acquires and refreshes credentials. Environment
  keys and auth commands use the admitted caller environment. A named custom
  route cannot use AWS signing, gateway OAuth, an incompatible managed
  policy or a built-in OpenAI remote-catalog override.
- **Catalogs, tiers, and errors**: custom routes keep upstream model IDs unchanged (unknown models
  need no OpenAI login/discovery) and leave unknown prices unknown. Service-tier choices scope by
  thread and exact model (`fast` maps to `priority`). Actual provider status/errors are preserved;
  sanitized metrics exclude credentials and raw error text.
- **Retries & rate-limit waits**: Codex owns retries. Recognized capacity
  errors honor `Retry-After` or exponential backoff (1s doubling to 30s); `usage_limit_reached`
  during a user turn waits until reported reset/`Retry-After` (or pauses for steer/interrupt)
  without consuming stream retries.

## Ownership

The app server admits the family, and the native model client sends requests
using its provider configuration. See
[configuration](../guide/configuration.md) for profile selection, precedence
and resume behavior.
