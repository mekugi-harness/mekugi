# Owner map

Use `docs/reference/modules.md` (generated) for feature modules, `docs/index.md` for specs and
contracts, and `graphify` or source search for file-level lookup. Native Rust runtime owners live in
`AGENTS.md#native-runtime` (built/tested via `runtime/build.py`; see
`docs/contributing/development.md#native-app-server`).

## Component owners

- **Native (`codex-rs`)**: family admission (app-server), root/child execution,
  coordination (`core/src/{agent,thread_manager}`), direct provider requests and
  usage (model client), retained edits (`change-history`, `exec-track`),
  model templates (`models-manager`), helper/child-reply guidance (`core/src/mekugi_guidance`),
  native helper tools (`mcat`, `mchanges`, `mfinspect`, `msymbol`, `mrun`), native shell guard
  (`codex-shell-guard`, `features.shell_guard`, `codex-arg0` guard shims, v2 `shellGuard/*`),
  context usage milestones (`token_budget.rs`), source acquisition & reload
  (`context_sources*.rs`, `compact_token_budget.rs`, `ext/skills`), OS/Git environment facts
  (`environment_facts.rs`), `SESSION_TMP` (`exec_env.rs`), and detached model-input projection
  (`context_manager/{output_deduplication,request_input}.rs`).
- **Go (`mekugi-tui/`)**: server/session lifetime, terminal UI,
  native event presentation, the frontend catalog, approval dock, session grants and
  private secret dialogs, retained session history, inspection, replay, usage/diagnostics.
- **Browser (`mekugi-webui/`)**: Svelte renderer, generated Session API types, and
  assets-only Go host. `mekugi-webui/DESIGN.md` owns visual and interaction rules.

## Application owners (`mekugi-tui/apps/mekugi/internal/`)

| Owner | Responsibility |
| --- | --- |
| `composition` | Selected descriptors/bindings; `contracts` holds shared interfaces; `testsupport/moduletest` holds harness |
| `launch` | CLI dispatch, profile selection, server/connector startup and native attachment |
| `frontends` | Authenticated executable catalog/PATH, worker pinning, and leases |
| `headless` | Noninteractive automation client and JSONL host evidence |
| `requestexecution` | Client failure notices and private storage diagnostics |
| `executionobservation` | User-shell capture and legacy edit review |
| `sessioncontrol` | Sole host app-server connection/request ledger; submission, compaction, approvals, prompts, presets, side threads |
| `sessionpresentation` | Scoped live/replay admission, turn/input/activity reduction, detached publications, metrics projection |
| `sessionapi` | Server-owned runtime admission, versioned Unix/WS control protocol, detached projections, HTTP resources |
| `connector` | Remote bearer-token/origin admission and WebSocket/HTTP forwarding to the local server |
| `terminalsession` | Terminal attachment, input precedence, geometry, frozen selection, detached views, and frame delivery |
| `sessionreview` | Authorized retained membership, async immutable review hydration, and rewind boundary selection |
| `recordtransactions` | Cross-owner publication, retained-input adoption, and fork initialization |
| `inspectioncli` | Offline `inspect-session`, `inspect-sessions`, and `inspect-storage` command dispatch |

## Key non-obvious boundaries

Go owner paths below are relative to `mekugi-tui/`; shared contracts are at root.

- `backends/codex/{appserver,binding,rollout,responses,shellhook,messagehistory,nativetrace}`
  owns Codex RPC, protocol decoding, rollout reads and shell hook protocols.
- `providers/openai` serves composer predictions. Inference uses the native model provider.
- `apps/mekugi/cmd/workspace` owns workspace enumeration, import/boundary checks, `make docs-check`,
  `make check-generated`, and `make integrate` (`docs/contributing/development.md#integrate-reviewed-work`).
- Shared cross-boundary contracts live in `docs/contracts/`: `coordination.md`, `composition.md`,
  `change-evidence.md`, `record-publication.md`, `responses-transport.md`, `provider-routing.md`.
- Controlled model comparisons live in https://github.com/yusing/codex-setup-ab (use only for comparison tasks).
