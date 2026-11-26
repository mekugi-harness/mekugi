// Source: apps/mekugi/cmd/mekugi/wrap.go:1:609@[ebde9ad5]
package launch

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strings"
	"syscall"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"golang.org/x/term"
)

func runWrap(routerArgs, args []string) int {
	if len(args) > 0 && args[0] == "codex" {
		args = args[1:]
	} else {
		routerArgs = append(slices.Clone(routerArgs), "custom-provider")
	}
	native := interactiveCodexArgs(args) && term.IsTerminal(int(os.Stdin.Fd())) && term.IsTerminal(int(os.Stdout.Fd()))
	signals := []os.Signal{syscall.SIGTERM}
	_, headless := headlessCommand(args)
	if native || headless {
		signals = append(signals, os.Interrupt)
	}
	ctx, stop := signal.NotifyContext(context.Background(), signals...)
	defer stop()
	code, err := wrapCodex(ctx, routerArgs, args)
	if err != nil {
		fmt.Fprintln(os.Stderr, "mekugi:", err)
	}
	return code
}

func wrapCodex(ctx context.Context, routerArgs, args []string) (code int, runErr error) {
	if headlessArgs, ok := headlessCommand(args); ok {
		return runHeadless(ctx, routerArgs, headlessArgs, os.Stdin, os.Stdout)
	}
	if interactiveCodexArgs(args) && term.IsTerminal(int(os.Stdin.Fd())) && term.IsTerminal(int(os.Stdout.Fd())) {
		if _, _, err := launchoptions.ProfileSelector(args); err != nil {
			return 2, err
		}
		stopProfiling, err := startProfiling(ctx, os.Stderr)
		if err != nil {
			return 1, err
		}
		defer func() { runErr = errors.Join(runErr, stopProfiling()) }()
		err = runNativeClient(ctx, "", routerArgs, args, os.Stdin, os.Stdout)
		if outcome, ok := errors.AsType[*sessionapi.NativeExit](err); ok {
			return outcome.Code, err
		}
		if err != nil {
			return 1, err
		}
		return 0, nil
	}
	if !interactiveCodexArgs(args) {
		return 2, errors.New("raw Codex subcommands are unsupported; run administrative commands directly with codex, or use mekugi codex headless --yolo for automation")
	}
	return 2, errors.New("interactive sessions require terminal stdin and stdout; use mekugi codex headless --yolo for automation")
}

// Reasoning shortcuts are session-local Codex config overrides. Preserve option
// operands and everything after -- rather than interpreting prompt contents.
func expandReasoningShortcuts(args []string) []string {
	if !interactiveCodexArgs(args) && (len(args) == 0 || args[0] != "headless") {
		return args
	}
	result := make([]string, 0, len(args))
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			return append(result, args[i:]...)
		}
		switch arg {
		case "--low", "--medium", "--high", "--xhigh", "--max", "--ultra":
			result = append(result, "-c", fmt.Sprintf("model_reasoning_effort=%q", strings.TrimPrefix(arg, "--")))
		case "-c", "--config", "--enable", "--disable", "-i", "--image", "-m", "--model", "-p", "--profile", "-s", "--sandbox", "-a", "--ask-for-approval", "-C", "--cd", "--add-dir":
			result = append(result, arg)
			if i+1 < len(args) {
				i++
				result = append(result, args[i])
			}
		default:
			result = append(result, arg)
		}
	}
	return result
}

const userBashEnvEnvironment = "MEKUGI_USER_BASH_ENV"
const frontendBashEnvMarker = "# Mekugi frontend shell startup\n"

// withoutInheritedFrontend removes invocation-owned shell state and restores
// the user's startup file before launching a child process.
func withoutInheritedFrontend(environment []string) []string {
	values := map[string]string{}
	for _, entry := range environment {
		key, value, _ := strings.Cut(entry, "=")
		values[key] = value
	}
	frontendDirectory := ""
	if path := values["BASH_ENV"]; filepath.Base(path) == "frontend-bash-env" {
		if body, err := os.ReadFile(path); err == nil && strings.HasPrefix(string(body), frontendBashEnvMarker) {
			frontendDirectory = filepath.Dir(path)
		}
	}
	vcsGuard, sudoGuard := values["CODEX_SHELL_GUARD_VCS_DIR"], values["CODEX_SHELL_GUARD_SUDO_DIR"]
	result := make([]string, 0, len(environment))
	for _, entry := range environment {
		key, value, _ := strings.Cut(entry, "=")
		if slices.Contains([]string{"CODEX_THREAD_ID", userBashEnvEnvironment, "MEKUGI_EXEC_TRACK", "MEKUGI_SH_TRACKER", "CODEX_SHELL_GUARD_SOCKET", "CODEX_SHELL_GUARD_VCS_DIR", "CODEX_SHELL_GUARD_SUDO_DIR"}, key) {
			continue
		}
		if key == "BASH_ENV" && frontendDirectory != "" {
			if previous := values[userBashEnvEnvironment]; previous != "" {
				result = append(result, "BASH_ENV="+previous)
			}
			continue
		}
		if key == "PATH" && (frontendDirectory != "" || vcsGuard != "" || sudoGuard != "") {
			paths := filepath.SplitList(value)
			paths = slices.DeleteFunc(paths, func(path string) bool {
				if path != "" && (path == vcsGuard || path == sudoGuard) {
					return true
				}
				if frontendDirectory == "" {
					return false
				}
				relative, err := filepath.Rel(frontendDirectory, path)
				return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator))
			})
			entry = "PATH=" + strings.Join(paths, string(os.PathListSeparator))
		}
		result = append(result, entry)
	}
	return result
}

func serverEnvironment(environment []string) []string {
	terminal := ""
	result := make([]string, 0, len(environment))
	for _, entry := range withoutInheritedFrontend(environment) {
		key, value, _ := strings.Cut(entry, "=")
		if key == "TERM" {
			terminal = value
		}
		if slices.Contains([]string{"MEKUGI_BASE_URL", "MEKUGI_RUNTIME_DIR", "TMUX", "TMUX_PANE", "TERM"}, key) {
			continue
		}
		result = append(result, entry)
	}
	if terminal == "" || terminal == "dumb" {
		terminal = "xterm-256color"
	}
	return append(result, "TERM="+terminal)
}

// Only session arguments may enter the native interactive client.
func interactiveCodexArgs(args []string) bool {
	for i := 0; i < len(args); i++ {
		arg := args[i]
		switch arg {
		case "--":
			return true
		case "-h", "--help", "-V", "--version":
			return false
		case "-c", "--config", "--enable", "--disable", "-i", "--image", "-m", "--model", "-p", "--profile", "-s", "--sandbox", "-a", "--ask-for-approval", "-C", "--cd", "--add-dir":
			i++
			continue
		}
		if strings.HasPrefix(arg, "-") {
			continue
		}
		switch arg {
		case "headless", "exec", "e", "review", "login", "logout", "mcp", "mcp-server", "app-server", "app", "completion", "sandbox", "debug", "apply", "a", "cloud", "features", "help":
			return false
		default:
			return true
		}
	}
	return true
}
