# Host acceptance

`codex_e2e`-tagged fixtures exercise `mekugi-app-server` and installed Codex
against local mock providers with fake credentials and isolated configuration.

Run `just build-native` to stage the runtime and its companions in `bin/native`.
Stock-CLI fixtures also need `codex` on `PATH`. If you run compaction fixtures
with raw `go test`, set `MEKUGI_E2E_MODEL` yourself.

## Running fixtures

Run the focused host suite: native `mrun` with piped input, PTY
yield/`write_stdin`, terminal steering and queues, and command tracking through
approval, workspace relocation, Dash, and the Linux sandbox. Nested `mchanges`
uses the stock CLI.

```sh
PATH="$PWD/bin/native:$PATH" just test-host
```

Check the native approval protocol separately:

```sh
just test-native --test-filter suite::v2::shell_guard
```

Compile all tagged launch fixtures (`mekugi-tui/apps/mekugi/internal/launch/*_codex_e2e_test.go`) without
running them:

```sh
just test ./mekugi-tui/apps/mekugi/internal/launch \
  -run '^$' -tags=codex_e2e
```

Report skips (such as Linux-only sandbox checks) as missing coverage. These local-mock fixtures do
not establish live-provider behavior; see [live Codex checks](testing-live-codex.md).
