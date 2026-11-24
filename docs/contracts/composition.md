# Workspace composition

## Requirements

Preserve [product outcomes and scope constraints](../product.md#user-requirements), stock execution,
bounded/batched reads, live diffs, and captured-change review. Structural rules are replaceable
means, not proof of better cost or quality.

## Boundary contract

- **Single workspace**: `mekugi-tui/go.work` is the membership authority. It lists the
  terminal client modules and the sibling `mekugi-webui` asset host module.
  The repository root has no Go module. Members ship together (`GOWORK=off` builds are unsupported).
- **Import boundaries** (enforced by `just workspace-check`):
  - `contracts` imports no local implementation.
  - `libraries` imports no application, feature, backend, or provider.
  - `backends` and `providers` stay mutually independent; production code never imports
    `testsupport`.
  - Feature domain/presentation imports only `contracts`, allowed `libraries`, and declared
    `Requires` module APIs, never another module's root, bindings, views, or internals. Only a
    feature's `codexbinding` imports `backends/codex/binding`.
  - Application `sessioncontrol`, `sessionpresentation`, `sessionapi`, and `headless`
    never import terminal libraries, feature terminal views, or `terminalsession`. The
    Web UI asset host does not depend on Go application code. `sessionapi` does
    not depend on the asset host; `frontends` never depends on `launch` or
    `composition`.
- **Single-source declarations**: each feature module declares `Module`, literal `Requires`
  (matching actual production imports), typed `Deps`, commands, record kinds, slots, and Codex
  connections once. Composition validates all descriptors before construction; startup acquires
  invocation resources in dependency order and drains in reverse order on shutdown or failure.
- **Generated module reference**: `docs/reference/modules.md` is derived from workspace
  membership, feature descriptors, and the app composition declarations without
  invoking constructors or callbacks. `just check-generated` detects drift;
  `just generate` refreshes the reference.

## Current approach (replaceable)

- Single-provider relationships use direct typed dependencies; shared extension families (status
  items, terminal views, inspection collectors, record kinds, reference resolvers) use typed slots
  whose hosts accept zero contributions.
- Both live execution and renderer-only socket clients consume the same `frontendDeclarations` list,
  with renderer-only clients stripping execution/store callbacks.
- Application-owned attachment, thread, and turn stores isolate module state via private typed keys
  and retire at scope boundaries.
