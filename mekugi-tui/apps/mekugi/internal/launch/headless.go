package launch

import (
	"context"
	"errors"
	"io"
	"os"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/headless"
	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
)

// runHeadless borrows the ordinary server runtime without a renderer or a
// detached daemon. The task owns admission, evidence delivery and family cleanup.
func runHeadless(ctx context.Context, routerArgs, args []string, input io.Reader, output io.Writer) (int, error) {
	profile, _, err := launchoptions.ProfileSelector(args)
	if err != nil {
		return 2, err
	}
	overrides, resume, yolo, err := appServerArgs(expandReasoningShortcuts(args))
	if err != nil {
		return 2, err
	}
	if err := validateProviderOverrides(overrides, profile); err != nil {
		return 2, err
	}
	if !yolo {
		return 2, errors.New("headless cannot answer approvals; it requires explicit --yolo")
	}
	if resume != "" {
		return 2, errors.New("headless runs a new thread; resume is not supported")
	}
	prompt, err := headless.ReadPrompt(ctx, input)
	if err != nil {
		return 1, err
	}
	workspace, err := os.Getwd()
	if err != nil {
		return 1, err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stream := headless.NewStream(output, cancel)
	launcher := &serverLauncher{headless: stream}
	hub := sessionapi.NewHub(ctx, launcher)
	launcher.hub = hub
	thread, err := hub.RunNative(ctx, sessionapi.NativeStart{Workspace: workspace, RouterArgs: routerArgs, Args: args, Environment: os.Environ()}, prompt)
	hub.Close()
	err = errors.Join(err, launcher.workers.Close(), stream.Err())
	if err != nil {
		return 1, err
	}
	if err := ctx.Err(); err != nil {
		return 1, err
	}
	if err := stream.Complete(thread); err != nil {
		return 1, err
	}
	return 0, nil
}

// The native argument parser has normalized config overrides to -c VALUE.
func validateProviderOverrides(args []string, profile string) error {
	for i := 0; i < len(args); i++ {
		if args[i] != "-c" {
			continue
		}
		i++
		key, _, _ := strings.Cut(args[i], "=")
		root, _, _ := strings.Cut(key, ".")
		switch strings.Trim(strings.TrimSpace(root), `"'`) {
		case "oss_provider":
			return errors.New("Mekugi does not support local OSS providers")
		case "model_provider", "model_providers", "openai_base_url":
			if profile == "" {
				return errors.New("provider overrides require a named Mekugi profile")
			}
		}
	}
	return nil
}
