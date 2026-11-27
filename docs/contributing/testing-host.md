# Host acceptance

`codex_e2e`-tagged fixtures exercise `mekugi-app-server` and installed Codex
against local mock providers with fake credentials and isolated configuration.

Run `just build-native` to stage the runtime and its companions in `bin/native`.
Separate stock-CLI fixtures need `codex` on `PATH`, but `test-host` does not.
If you run compaction fixtures with raw `go test`, set `MEKUGI_E2E_MODEL`
yourself.

## Running fixtures

Run the focused host suite against the managed runtime. It
checks terminal resume, pickers, steering and queues, approval denial,
side questions, one-off execution, and manual titles. Managed-runtime cases
also exercise native `mrun` with piped input, PTY yield/`write_stdin`, durable
change history through `mchanges`, and command execution through a user hook,
workspace relocation, Dash, and both restricted Linux sandbox modes.

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
