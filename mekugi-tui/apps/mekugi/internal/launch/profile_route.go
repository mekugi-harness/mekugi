package launch

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
)

// Context's private environment is admitted over the native control channel;
// children borrow threadSetup instead. Never mutate the daemon environment.
type invocationEnvironmentKey struct{}

func invocationEnvironment(ctx context.Context) []string {
	if environment, ok := ctx.Value(invocationEnvironmentKey{}).([]string); ok {
		return slices.Clone(environment)
	}
	return os.Environ()
}

func invocationExecutable(name string, environment []string, workspace string) (string, error) {
	directories := filepath.SplitList(launchoptions.EnvironmentValue(environment, "PATH"))
	if name == "mekugi-app-server" {
		if executable, err := os.Executable(); err == nil {
			directories = append(directories, filepath.Dir(executable))
		}
	}
	for _, directory := range directories {
		if !filepath.IsAbs(directory) {
			directory = filepath.Join(workspace, directory)
		}
		path, err := filepath.Abs(filepath.Join(directory, name))
		if err != nil {
			continue
		}
		if info, err := os.Stat(path); err == nil && !info.IsDir() && info.Mode()&0111 != 0 {
			return path, nil
		}
	}
	return "", exec.ErrNotFound
}

func headlessCommand(args []string) ([]string, bool) {
	for i := 0; i < len(args); i++ {
		if args[i] == "--" {
			break
		}
		if args[i] == "headless" {
			return slices.Delete(slices.Clone(args), i, i+1), true
		}
		if args[i] == "-p" || args[i] == "--profile" || launchoptions.CodexOptionOperand(args[i]) {
			i++
		} else if !strings.HasPrefix(args[i], "-") {
			break
		}
	}
	return nil, false
}
