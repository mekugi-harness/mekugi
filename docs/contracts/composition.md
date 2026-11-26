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
  - Feature modules import only `contracts`, `libraries`, and public packages of sibling
    feature modules (never another module's root, `internal`, `codexbinding`,
    `terminalview`, or `presentation`). Only a feature's `codexbinding` imports
    `backends/codex/binding`.
  - Application `sessioncontrol`, `sessionpresentation`, `sessionapi`, and `headless`
    never import terminal libraries, feature terminal views, or `terminalsession`. The
    Web UI asset host does not depend on Go application code. `sessionapi` does
    not depend on the asset host; `frontends` never depends on `launch` or
    `composition`.
- **Generated module reference**: `docs/reference/modules.md` is derived from workspace
  membership in `mekugi-tui/go.work`, each module's `SPEC.md` summary, and its Go
  packages. `just check-generated` detects drift;
  `just generate` refreshes the reference.

## Current approach (replaceable)

- Each module under `mekugi-tui/modules/` documents its scope in `SPEC.md` and exposes
  presentation/terminal view packages directly to the TUI application.
