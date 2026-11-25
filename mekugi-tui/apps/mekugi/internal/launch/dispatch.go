// Source: apps/mekugi/cmd/mekugi/main.go:1:86@[ebde9ad5]
package launch

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
)

// Source: main.go:36:48 process signals and router exit behavior.
// Run dispatches the process invocation and returns its exit status.
func Run() (exitCode int) {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if len(os.Args) > 1 {
		switch os.Args[1] {
		case "connector":
			return RunConnector(ctx, os.Args[2:], os.Stderr)
		case "web":
			return RunWeb(ctx, os.Args[2:], os.Stderr)
		}
	}
	routerArgs, command, err := launchoptions.SplitCommand(os.Args[1:])
	if errors.Is(err, flag.ErrHelp) {
		launchoptions.PrintUsage(os.Stdout)
		return 0
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "mekugi:", err)
		return 2
	}
	stop()
	return runWrap(routerArgs, command)
}
