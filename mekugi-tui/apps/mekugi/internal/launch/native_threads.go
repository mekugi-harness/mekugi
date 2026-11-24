package launch

import (
	"context"
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/executionobservation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/headless"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

// threadExecution joins the supplied runtime registry and feature owner. It owns
// no process registry, request ledger, or retained thread state.
type threadExecution struct {
	native       *nativeConnection
	headless     *headless.Stream
	workers      *frontends.WorkerImage
	sourceSocket string
	hub          *sessionapi.Hub

	features *sessioncontrol.NativeFeatureUse
	retained changes.Scope
	windows  *executionobservation.Windows
	tools    []binding.MCPServer
	setup    *threadSetup
}

// The invoking runtime freezes configuration before child admission. Descendants
// borrow this selection, not subsequently changed user configuration or argv.
type threadSetup struct {
	options     launchoptions.Config
	settings    launchoptions.Settings
	executable  string
	args        []string
	environment []string
	approvals   bool
}

func (e *threadExecution) Runtime(thread string) (sessionapi.NativeRuntime, error) {
	runtime, err := e.hub.Requests(thread)
	if err == nil || e.native == nil {
		return runtime, err
	}
	// Native execution belongs to the shared connection, not to a hydrated UI
	// endpoint. Each operation validates its target on the serialized owner.
	return e.native.root.Client.Requests, nil
}

// AttachChild attaches only a native-admitted descendant on the shared connection. Selection
// does not reserve a work generation, dispatch input, or wake a retained wait.
func (e *threadExecution) AttachChild(ctx context.Context, parent, thread string) error {
	if e.native == nil {
		return errors.New("native child connection is unavailable")
	}
	return e.native.attachChild(ctx, parent, thread)
}
