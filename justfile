set quiet
set positional-arguments
export CODEX_REPO_ROOT := justfile_directory()
export GOWORK := justfile_directory() / 'mekugi-tui/go.work'

just := quote(just_executable())
workspace := 'go run -trimpath ./mekugi-tui/apps/mekugi/cmd/workspace'
go_commands := './mekugi-tui/apps/mekugi/cmd/mekugi ./mekugi-tui/apps/mekugi/cmd/mekugi-exec'
launch := './mekugi-tui/apps/mekugi/internal/launch'
native_files := 'mekugi-app-server codex-code-mode-host' + if os() == 'linux' { ' bwrap' } else { '' }
go_bin := 'dir=$(go env GOBIN); if [ -z "$dir" ]; then dir=$(go env GOPATH); dir=${dir%%:*}/bin; fi'
rtk := env('RTK', shell('command -v rtk >/dev/null && printf rtk || :'))
tool_output := if rtk == '' { '' } else { rtk + ' err' }
test_output := if rtk == '' { '' } else { rtk + ' test' }
test_mode := if rtk == '' { 'run' } else { 'test' }
go_test := workspace + ' ' + test_mode + ' go test -trimpath'

[private]
default:
    {{ just }} --list

# Check imports and build the Go workspace, without building browser assets.
build: workspace-check
    {{ workspace }} run go build -trimpath @workspace

# Stage the client, embedded browser UI and native runtime in bin/.
build-binaries: webui build
    go build -trimpath -ldflags='-s -w' -o bin/ {{ go_commands }}
    python3 runtime/build.py build --output bin

install: workspace-check webui build-native
    go install -trimpath {{ go_commands }}
    {{ go_bin }}; (cd bin/native && install -m 755 {{ native_files }} "$dir/" && install -m 644 mekugi-models.json "$dir/")

uninstall:
    go clean -i {{ go_commands }}
    {{ go_bin }}; for file in {{ native_files }} mekugi-models.json; do rm -f "$dir/$file"; done

workspace-check:
    {{ workspace }} check

docs-check:
    {{ workspace }} docs-check

# Forward integration options and check argv as supplied.
integrate +args:
    {{ workspace }} integrate "$@"

# Pass native build options directly; Cargo selectors go after --.
build-native *args:
    python3 runtime/build.py build "$@"

check-native *args:
    {{ just }} _test-env python3 runtime/build.py check "$@"

test-native *args:
    {{ just }} _test-env python3 runtime/build.py test "$@"

package-native *args:
    python3 runtime/build.py package "$@"

# Plain Go builds and tests can run without the browser bundle.
[working-directory('mekugi-webui/app')]
webui: _webui-deps
    {{ tool_output }} bun run --silent build

[working-directory('mekugi-webui/app')]
webui-check: _webui-deps
    {{ test_output }} bun run --silent test
    {{ tool_output }} bun run --silent check
    {{ tool_output }} bun run --silent lint
    {{ tool_output }} bun run --silent format:check

[working-directory('mekugi-webui/app')]
webui-fmt: _webui-deps
    {{ tool_output }} bun run --silent format

[working-directory('mekugi-webui/app')]
_webui-deps:
    {{ rtk }} bun install --frozen-lockfile --no-progress --no-summary

generate: workspace-check
    {{ workspace }} index-write
    go run ./mekugi-tui/apps/mekugi/cmd/sessionapi-schema

check-generated:
    {{ workspace }} index-check
    go run ./mekugi-tui/apps/mekugi/cmd/sessionapi-schema -check

# Diagnostic build, with symbols and the same optimized code as production.
mekugi-pprof:
    go build -trimpath -tags pprof -o bin/mekugi-pprof ./mekugi-tui/apps/mekugi/cmd/mekugi
    go build -trimpath -o bin/mekugi-exec ./mekugi-tui/apps/mekugi/cmd/mekugi-exec

# Real terminal preview with an isolated, scripted local Responses provider.
[script('sh')]
preview-native-ui:
    set -eu
    preview_bin=$(mktemp -d)
    trap 'rm -rf "$preview_bin"' EXIT
    preview_welcome=$(awk '
        /^## / { if (preview) exit; preview = ($0 == "## Preview the terminal UI") }
        !preview { next }
        /^($|## |- )/ { if (line != "") print line; line = $0; if (line == "") print ""; next }
        { sub(/^ +/, ""); line = line (line == "" ? "" : " ") $0 }
        END { if (line != "") print line }' docs/contributing/development.md)
    test -n "$preview_welcome"
    go build -trimpath \
        -ldflags="-X \"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession.previewWelcome=$preview_welcome\"" \
        -o "$preview_bin/" {{ go_commands }}
    PATH="$PWD/bin/native:$PATH" MEKUGI_PREVIEW_BINARY="$preview_bin/mekugi" \
        {{ just }} _test-env go test -trimpath \
        {{ launch }} -run '^TestNativeUIWalkthrough$' -count=1 -timeout 0

# Defaults to all workspace packages. Otherwise pass packages and go test flags.
test *args='@workspace': workspace-check
    MEKUGI_E2E_MODEL="${MEKUGI_E2E_MODEL-gpt-6-luna}" {{ just }} _test-env {{ go_test }} -parallel=32 "$@"

# Managed runtime and stock Codex against isolated local mocks.
[script('sh')]
test-host:
    set -eu
    {{ just }} _test-env codex --version
    set -- TestAppServerNativeMRunAndPTY \
        TestMChangesNestedNativeCodexE2E \
        TestAppServerExecTrackNativeCodex \
        TestAppServerExecTrackNativeCodexDash \
        TestAppServerExecTrackNativeCodexMovedWorkspace \
        TestAppServerExecTrackNativeCodexSandbox \
        TestAppServerSteerAndQueueNativeCodex
    pattern=$(IFS='|'; printf '^(%s)$' "$*")
    if ! available=$({{ just }} _test-env go test -trimpath -tags=codex_e2e {{ launch }} -list "$pattern"); then
        printf '%s\n' "$available"
        exit 1
    fi
    for name do
        printf '%s\n' "$available" | grep -Fxq "$name" || { printf 'Missing host acceptance test: %s\n' "$name" >&2; exit 1; }
    done
    {{ just }} test {{ launch }} -run "$pattern" -tags=codex_e2e -count=1 -v

[script('sh')]
lint: workspace-check
    set -eu
    unset BASH_ENV
    {{ workspace }} run golangci-lint run --allow-serial-runners @workspace
    findings=$({{ workspace }} run deadcode -f='{{{{range .Funcs}}{{{{printf "%s.%s\n" $.Path .Name}}{{{{end}}' @workspace)
    findings=$(printf '%s\n' "$findings" | awk 'FILENAME == ARGV[1] { allowed[$0] = 1; next } /^github.com\/mekugi-harness\/mekugi\/mekugi-tui\/testsupport\// { next } NF && !($0 in allowed)' .deadcode-allowlist -)
    if [ -n "$findings" ]; then printf '%s\n' "$findings"; exit 1; fi
    findings=$({{ workspace }} run deadcode -test @workspace)
    if [ -n "$findings" ]; then printf '%s\n' "$findings"; exit 1; fi

# Compare rendered output. Mismatches leave .txt.new candidates.
test-ui-snapshots packages='@snapshots' pattern='^TestUISnapshot':
    {{ just }} _test-env {{ go_test }} ./mekugi-tui/testsupport/uisnapshot/... "$1" -run "TestSnapshot|$2" -count=1

# Replace baselines for the selected snapshot tests.
update-ui-snapshots packages='@snapshots' pattern='^TestUISnapshot':
    {{ just }} _test-env env MEKUGI_UPDATE_UI_SNAPSHOTS=1 {{ go_test }} "$1" -run "$2" -count=1

# Do not let an active Mekugi session intercept tests or taint the test cache.
[script('sh')]
_test-env +command:
    set -eu
    clean_path=$(printf '%s:' "$PATH" | awk -v RS=: -v ORS=: -v startup="${BASH_ENV-}" '
        BEGIN {
            if (startup ~ /\/frontend-bash-env$/) {
                sub(/\/frontend-bash-env$/, "", startup)
                frontend = startup
            }
        }
        !(frontend && ($0 == frontend || index($0, frontend "/") == 1)) &&
            ($0 == "" || ($0 != ENVIRON["CODEX_SHELL_GUARD_VCS_DIR"] && $0 != ENVIRON["CODEX_SHELL_GUARD_SUDO_DIR"]))
    ' | sed 's/:$//')
    exec env -u BASH_ENV -u COLORFGBG -u CODEX_THREAD_ID -u CODEX_ROLLOUT_TRACE_ROOT \
        -u CODEX_SHELL_GUARD_SOCKET -u CODEX_SHELL_GUARD_VCS_DIR -u CODEX_SHELL_GUARD_SUDO_DIR \
        -u CODEX_SHELL_GUARD_SUDO_COMMAND -u MEKUGI_AX_OUTPUT -u MEKUGI_BASE_URL \
        -u MEKUGI_CONTROL_SOCKET -u MEKUGI_EXEC_TRACK -u MEKUGI_SH_TRACKER \
        -u MEKUGI_UPDATE_UI_SNAPSHOTS -u MEKUGI_USER_BASH_ENV -u MEKUGI_VCS_GUARD_HOOK \
        TMPDIR="$(cd "${TMPDIR:-/tmp}" && pwd -P)" PATH="$clean_path" "$@"

# Rust maintenance excludes upstream clients, SDKs and Bazel packaging.
fmt:
    python3 scripts/format.py

fmt-check:
    python3 scripts/format.py --check

[working-directory('codex-rs')]
fix *args:
    cargo clippy --fix --tests --allow-dirty "$@"

[working-directory('codex-rs')]
clippy *args:
    cargo clippy --tests "$@"

[working-directory('codex-rs')]
test-rust *args:
    RUST_MIN_STACK=8388608 NEXTEST_PROFILE=local cargo nextest run --no-fail-fast "$@"

[working-directory('codex-rs')]
write-config-schema:
    cargo run -p codex-config-schema --bin codex-write-config-schema

# Regenerate native protocol artifacts without either SDK.
[working-directory('codex-rs')]
write-app-server-schema *args:
    python3 app-server-protocol/scripts/write_schema_fixtures.py "$@"

[working-directory('codex-rs')]
write-hooks-schema:
    cargo run -p codex-hooks --bin write_hooks_schema_fixtures
