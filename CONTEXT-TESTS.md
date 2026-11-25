# Testing

Load a specialized guide only when that operation applies:

| Operation | Guide |
| --- | --- |
| Native Rust runtime checks/tests | `AGENTS.md#native-validation-and-generation` |
| Installed-Codex `codex_e2e` fixtures (local mock providers) | `docs/contributing/testing-host.md` |
| Live-provider `e2e` fixtures | `docs/contributing/testing-live-codex.md` |
| Terminal UI snapshots | `docs/contributing/testing-snapshots.md` |
| Browser control transport fixture & wire types | `docs/contributing/testing-browser-control.md` |
| Benchmarks and test CPU/block profiles | `docs/contributing/testing-performance.md` |

## Runner

Use `just test` from the repository root (root has no Go module):

```sh
just test ./mekugi-tui/backends/codex/appserver \
  ./mekugi-tui/apps/mekugi/internal/launch -run AppServer
```

- Bare `just test` checks all `go.work` members. With arguments, pass package
  paths or `@workspace` and ordinary `go test` flags. Set `RTK=` for raw output.
  The default `-parallel=32` overlaps process/PTY waits; keep env/cwd-mutating
  tests serial.
- Use `just affected` (optionally `just affected <base>` or `just affected --dry-run`)
  for fast path-scoped local verification against `origin/main` plus working-tree
  edits: it skips Cargo when `codex-rs/` is untouched, runs `just webui-check`
  only when `mekugi-webui/` changes, scopes `just test` to touched Go packages,
  and runs `just test-rust --locked` scoped to touched Rust crates (falling back
  to `just test-native` for workspace-level Rust manifest/lockfile changes).
- Prefer `just test` inside an active Mekugi shell: it drops per-session
  variables and frontend `PATH` entries to bypass live hooks and reuse cached
  results. Raw `go test` needs at least `env -u BASH_ENV -u MEKUGI_EXEC_TRACK`.
- Tests neither generate assets nor install binaries. Interpreter/query fixtures need Node.js and `ripgrep`.
- CI runs the offline suite on Linux amd64; other platforms run command/version checks.
- Report wall time for full runs (cold build/race/live separately); do not rerun passing tests to chase the ~30s warm-cache target.

## Coverage & static checks

- **Native vs Go E2E**: when moving behavior to `codex-rs` or adding a native-binary Go E2E test, check Rust coverage first. Put native behavior tests in Rust and keep Go E2E only for distinct Go-side integration.
- **Static checks**:
  - `just docs-check`: checks Markdown links and anchors (also in pre-commit).
  - `just lint`: runs `golangci-lint` and `deadcode` before committing Go changes.
  - `just check-generated`: checks the module reference and browser Session API
    types. Run `just generate` to refresh both.
  - `just preview-native-ui`: runs Mekugi against a scripted local provider.
    See [Preview the terminal UI](docs/contributing/development.md#preview-the-terminal-ui).
- **Opt-in focused checks** (never install missing tools):
  - Helper guidance (native and wrapped launcher):

    ```sh
    just test-native -- -p codex-core --lib -- mekugi_guidance::tests
    just test ./mekugi-tui/apps/mekugi/internal/frontends \
      ./mekugi-tui/apps/mekugi/internal/launch \
      -run 'Guidance|ComposedFrontendCatalog|ToolFrontend'
    ```
- After API changes, compile tagged host/live fixtures (`-run '^$'` with
  `-tags=codex_e2e` or `-tags=codex_e2e,e2e`).

## Fixture gotchas

- Go's test cache tracks only files inside the test's own module. Read files
  like `justfile` and `.githooks` through `testdata` symlinks so edits invalidate
  cached results. Never `t.Chdir` into the repository root.
- Isolate mutable thread/workspace/process state; reuse immutable registries. Disposable Git fixtures must isolate global/system config to avoid personal hooks/signing.
- Resolve fixture shells via `execTrackShellExecutable` (`mekugi-tui/apps/mekugi/internal/launch/exec_track_test.go`): `exec.Command` resolves executables before `Cmd.Env`, so env isolation alone can hit the live approval guard.
- Use controlled time for lifetimes/locks; keep subtests and parallel scheduling outside `synctest` bubbles (create/clean up workers inside).
