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
- **Go (`mekugi-tui/`)**: terminal UI renderer (keys, local editing, geometry, painting),
  direct native `mekugi-app-server` Unix-socket client, headless client,
  and remote connector/WebUI assets host (`connector`, `web`).
- **Browser (`mekugi-webui/`)**: Svelte renderer, native protocol types imported from
  `codex-rs/app-server-protocol/schema/typescript`, and
  assets-only Go host. `mekugi-webui/DESIGN.md` owns visual and interaction rules.

## Application owners (`mekugi-tui/apps/mekugi/internal/`)

| Owner | Responsibility |
| --- | --- |
| `composition` | Renderer-only module view selection (`NativeRenderer`); `contracts` holds shared interfaces; `testsupport/moduletest` holds harness |
| `launch` | CLI dispatch, profile selection, direct native `mekugi-app-server` startup/attachment, connector, web, and headless |
| `headless` | Noninteractive automation client and JSONL host evidence |
| `sessioncontrol` | Minimal headless request correlation and shared interrupt/thread types |
| `sessionpresentation` | Detached presentation view/intent types and native replay projection |
| `sessionapi` | Wire protocol constants and peer-checked Unix control transport |
| `connector` | Remote bearer-token/origin admission and WebSocket/HTTP forwarding to the local native `mekugi-app-server` socket |
| `terminalsession` | Terminal attachment, input precedence, geometry, frozen selection, detached views, and frame delivery |
| `inspectioncli` | Native `mekugi-app-server` CLI dispatch for `inspect-session`, `inspect-sessions`, and `inspect-storage` |

## Key non-obvious boundaries

Go owner paths below are relative to `mekugi-tui/`; shared contracts are at root.

- `backends/codex/{appserver,binding,rollout,responses,shellhook,messagehistory,nativetrace}`
  owns Codex RPC, protocol decoding, rollout reads and shell hook protocols.
- `apps/mekugi/cmd/workspace` owns workspace enumeration and import checks,
  plus `just docs-check`, the module index, and `just integrate`
  (`docs/contributing/development.md#integrate-reviewed-work`).
- Shared cross-boundary contracts live in `docs/contracts/`: `coordination.md`, `composition.md`,
  `change-evidence.md`, `record-publication.md`, `responses-transport.md`, `provider-routing.md`.
- Controlled model comparisons live in https://github.com/yusing/codex-setup-ab (use only for comparison tasks).
