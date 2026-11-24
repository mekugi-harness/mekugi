package launch

import (
	"context"
	"errors"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/executionobservation"
	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/nativetrace"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	activitymodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/reports"
	segments "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/segments"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences/preferences"
	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
)

// appServerIntegration selects independent collaborators for native-session fixtures.
type appServerIntegration struct {
	trackingCommand     string
	preparer            *executionobservation.Preparer
	controlSocket       chan<- string
	controlObservations *sessioncontrol.Observations
	usage               *accounting.Service
	audience            *sessioncontrol.Audience
	review              *sessionreview.Review
	execWindows         *executionobservation.Windows
	nativeTrace         *nativetrace.Reader
	retained            changes.Scope
	changeSource        changes.Service
	snapshots           *changecapture.Workspaces
	publishChanges      func([]changes.Update)
	captureNotice       func(string, string, string, string)
	execTrack           *segments.Observer
	skillsManager       bool
}

func startAppServerUI(terminalViews terminalsession.Views, reviewSource *changecapture.Service, views *sessionpresentation.Service, conversation *conversationmodel.Service, activity *activitymodel.Service, service *history.Service, prefs *preferences.Service, segments *reports.Service, ctx context.Context, cmd *exec.Cmd, stdin, stdout *os.File, integration appServerIntegration, issues *requestexecution.Failures, resumeThread string, faint bool, capture *transportcapture.Recorder, resumeArgv []string, debugDirectory string, observations *binding.Observation, approvals bool) (func() error, error) {
	workspace, err := filepath.Abs(cmd.Dir)
	if err != nil {
		return nil, err
	}
	if integration.retained.Store == nil {
		directory, err := history.ReplayDirectory()
		if err != nil {
			return nil, err
		}
		integration.retained.Store, err = service.Open(ctx, directory)
		if err != nil {
			return nil, err
		}
		retainedChanges, err := changecapture.New(service, directory)
		if err != nil {
			return nil, err
		}
		integration.changeSource = retainedChanges.Service
	}
	return func() (runErr error) {
		directory, err := os.MkdirTemp("", "native-fixture-")
		if err != nil {
			return err
		}
		defer os.RemoveAll(directory)
		socket := filepath.Join(directory, "s")
		listener, err := sessionapi.ListenControl(socket)
		if err != nil {
			return err
		}
		defer listener.Close()
		serverContext, stopServer := context.WithCancel(ctx)
		launcher := nativeFixtureLauncher{run: func(runtimeContext context.Context, api *sessionapi.Server, threads *threadExecution) error {
			wait, err := (&nativeSession{trackingCommand: integration.trackingCommand, preparer: integration.preparer, reviewSource: reviewSource, presentationState: views, conversationState: conversation, activity: activity, history: service, segments: segments, ctx: runtimeContext, controlObservations: integration.controlObservations, usage: integration.usage, audience: integration.audience, review: integration.review, execWindows: integration.execWindows, nativeTrace: integration.nativeTrace, retained: integration.retained, changeSource: integration.changeSource, snapshots: integration.snapshots, publishChanges: integration.publishChanges, captureNotice: integration.captureNotice, execTrack: integration.execTrack, issues: issues, sessionCapture: capture, resumeArgv: resumeArgv, replayDebugDirectory: debugDirectory, observations: observations, approvalMode: approvals}).start(prefs, cmd, integration.skillsManager, resumeThread, nil, api, launchoptions.Config{}, threads)
			if err != nil {
				return err
			}
			return wait()
		}}
		hub := sessionapi.NewHub(serverContext, &launcher)
		launcher.hub = hub
		if integration.controlSocket != nil {
			integration.controlSocket <- socket
		}
		done := make(chan error, 1)
		go func() { done <- serveSessionHub(serverContext, listener, hub) }()
		defer func() { stopServer(); runErr = errors.Join(runErr, <-done) }()
		transport := sessionapi.ControlTransport(socket)
		defer transport.CloseIdleConnections()
		connection, err := sessionapi.StartNative(ctx, &http.Client{Transport: transport}, "http://mekugi", sessionapi.NativeStart{Workspace: workspace})
		if err != nil {
			return err
		}
		defer connection.Close()
		terminal, _ := terminalViews.NewAttachment(diffrender.EnvironmentTheme(os.Getenv("COLORFGBG")))
		terminal.Faint = faint
		client := nativeClient{connection: connection, renderer: nativeClientRenderer{terminal: terminal, views: terminalViews}, editors: make(map[string]*composer.Editor)}
		_, err = client.run(ctx, stdin, stdout)
		if outcome, ok := errors.AsType[*sessionapi.NativeExit](err); ok && outcome.Code == 0 {
			return nil
		}
		return err
	}, nil
}

// Fixtures inject their existing host/owner collaborators while using the real
// socket, runtime admission and native renderer. The fixture owns server teardown.
type nativeFixtureLauncher struct {
	serverLauncher
	run func(context.Context, *sessionapi.Server, *threadExecution) error
}

func (l *nativeFixtureLauncher) Run(ctx context.Context, _ sessionapi.Start, api *sessionapi.Server) error {
	threads := &threadExecution{hub: l.hub, setup: &threadSetup{}}
	return l.run(ctx, api, threads)
}

// Real server fixtures use the same admitted owners as production startup.
func newTestServerHub(ctx context.Context) *sessionapi.Hub {
	launcher := &serverLauncher{}
	hub := sessionapi.NewHub(ctx, launcher)
	launcher.hub = hub
	return hub
}
