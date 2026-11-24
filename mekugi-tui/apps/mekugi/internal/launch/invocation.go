package launch

import (
	"context"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	history "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/shellhook"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/shellinstrumentation"
	segmenttransport "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/transport"

	"io"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/executionobservation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/nativetrace"

	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"

	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"

	activitymodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences/preferences"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/command"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"

	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"

	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/providers/openai"

	segmentservice "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/segments"

	// Source: main.go:20:551 HTTP lifecycle and Responses proxy execution.

	changehistory "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

// runInvocation acquires independent owners, launches Codex and joins every lifetime.
// artifacts receives retained debug paths at completion, even if startup was canceled.
func runInvocation(views *sessionpresentation.Service, conversation *conversationmodel.Service, live *previewmodel.Service, activity *activitymodel.Service, service *history.Service, prefs *preferences.Service, segments *segmentservice.Service, changes *changecapture.Service, usage *accounting.Service, agentMetrics *evidence.Service, diagnostics *transportcapture.Service, diagnosis *diagnostic.Service, ctx context.Context, commands []command.Contribution, autoTitle bool, observations *binding.Observation, options launchoptions.Config, config launchoptions.Settings, issues *requestexecution.Failures, executable string, args, environment []string, stderr io.Writer, resumeThread string, resumeArgv []string, approvals bool, artifacts func([]string), api *sessionapi.Server, workspace string, threads *threadExecution) (code int, runErr error) {
	if api == nil || threads == nil {
		return 1, errors.New("native invocation requires control API and thread execution owners")
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	defer func() {
		if runErr != nil && code == 0 {
			code = 1
		}
	}()
	if diagnostics == nil && options.CaptureOutput != "" {
		return 1, errors.New("transport diagnostics are unavailable")
	}
	var readLogPath string
	for _, entry := range environment {
		if value, ok := strings.CutPrefix(entry, evidence.AXReadOutputEnvironment+"="); ok {
			readLogPath = value
		}
	}
	if agentMetrics != nil {
		if err := evidence.ValidateOutputAliases(readLogPath, options.CaptureOutput); err != nil {
			return 1, err
		}
	}
	debug, err := openDebugOutput(diagnosis, options.Debug, options.CaptureOutput, readLogPath)
	if err != nil {
		return 1, err
	}
	var mekugiCalls *executionobservation.Preparer
	var registry *frontends.Registry
	var retained changehistory.Scope
	var changeSource changehistory.Service
	var checkpoints *changecapture.Workspaces
	var audience *sessioncontrol.Audience
	var review *sessionreview.Review
	var windows *executionobservation.Windows
	var nativeTrace *nativetrace.Reader
	var control sessioncontrol.Observations
	var reportObserver *segmentservice.Observer
	defer func() {
		if debug != nil {
			debug.Event(map[string]any{"event": "router_stop", "failed": runErr != nil, "storage_writes": debug.Writes().Snapshot()})
			runErr = errors.Join(runErr, debug.Close())
		}
		if artifacts != nil {
			var paths []string
			if debug != nil {
				paths = append(paths, debug.Paths()...)
			}
			if len(paths) != 0 {
				artifacts(paths)
			}
		}
	}()
	var writes *persistence.Counter
	if debug != nil {
		writes = debug.Writes()
	}
	capturePath := options.CaptureOutput
	if debug != nil {
		capturePath = debug.CapturePath()
	}
	var capture *transportcapture.Recorder
	if diagnostics != nil {
		if capturePath != "" {
			capturePath, err = filepath.Abs(capturePath)
			if err != nil {
				return 1, fmt.Errorf("resolve capture output: %w", err)
			}
		}
		captureConfig := transportcapture.Config{Output: capturePath, Writes: writes}
		capture, err = diagnostics.Open(captureConfig)
		if err != nil {
			return 1, fmt.Errorf("initialize capture: %w", err)
		}
	}
	var metricsFile *os.File
	defer func() {
		if capture != nil {
			runErr = errors.Join(runErr, capture.Close())
		}
		if metricsFile != nil {
			runErr = errors.Join(runErr, capture.WriteMetrics(writes.Writer(metricsFile)), metricsFile.Close())
		}
	}()
	if debug != nil && capture != nil {
		metricsFile, err = os.OpenFile(debug.MetricsPath(), os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600)
		if err != nil {
			return 1, fmt.Errorf("open metrics output: %w", err)
		}
	}

	var predictionProvider *openai.Client
	if options.Profile == "" {
		predictionProvider = openai.New(openai.BaseURL, nil)
	}

	var frontendDirectory string
	if issues != nil {
		issues.Configure(requestexecution.FailureStorage{Diagnostics: diagnosis, Writes: writes})
	}
	replayDirectory, err := history.ReplayDirectory()
	if err != nil {
		return 1, fmt.Errorf("initialize replay storage: %w", err)
	}
	registry, err = frontends.New(ctx, frontends.Config{Commands: commands, ReplayDirectory: replayDirectory, WorkerImage: threads.workers})
	if err != nil {
		return 1, fmt.Errorf("initialize tool registry: %w", err)
	}
	defer func() {
		runErr = errors.Join(runErr, registry.Close())
	}()
	frontendDirectory = registry.FrontendDirectory
	retained.Store, err = service.Open(ctx, replayDirectory)
	if err != nil {
		return 1, fmt.Errorf("initialize replay storage: %w", err)
	}
	checkpoints, err = changes.Checkpoints(retained.Store.Directory())
	if err != nil {
		return 1, fmt.Errorf("initialize replay checkpoints: %w", err)
	}
	if changes != nil {
		changeSource = changes.Service
	}
	retained.Writes = writes
	retained.Limits = history.Limits{Total: history.DefaultStorageBytes, Kinds: map[string]int64{"commentary": 16 << 20}}
	threads.retained = retained
	audience = sessioncontrol.NewAudience(func(category, message string) { issues.AddThreadNotice("", "", category, message) })
	windows = &executionobservation.Windows{}
	if threads.windows == nil {
		threads.windows = windows
	}
	windows = threads.windows
	traceDirectory, traceErr := os.MkdirTemp("", "mekugi-native-trace-")
	if traceErr == nil {
		nativeTrace = nativetrace.NewWithNotice(traceDirectory, func(message string) {
			issues.AddNotice("", "native_trace_format", message)
		})
		defer func() { runErr = errors.Join(runErr, os.RemoveAll(traceDirectory)) }()
	} else {
		issues.AddNotice("", "native_trace", "Nested tool confirmation unavailable: "+traceErr.Error())
	}
	review = sessionreview.New(ctx, live, func(category, message string) { issues.AddNotice("", category, message) })
	defer review.Close()
	mekugiCalls = executionobservation.New(executionobservation.Config{ProgressSnapshot: threads.hub.ProgressSnapshot, Review: review, Audience: audience, Windows: windows, NativeTrace: nativeTrace, Retained: retained, Changes: changeSource, Checkpoints: checkpoints, PublishChanges: review.Publish, Notice: issues.AddThreadNotice})
	usage.Configure(retained.Store, writes, retained.Limits, func(thread string, err error) {
		issues.AddThreadNotice("", thread, "usage_storage", "Historical provider usage could not be restored: "+err.Error())
	})
	if issues != nil {
		issues.Configure(requestexecution.FailureStorage{Diagnostics: diagnosis, Writes: writes, Store: retained.Store, Limits: retained.Limits})
	}
	defer func() {
		windows.Retire()
		if reportObserver != nil {
			reportObserver.Close()
		}
		audience.Close()
	}()

	skillsManagerAvailable := skillsManagerInPath(environment, workspace, frontendDirectory)

	retentionCtx, stopRetention := context.WithCancel(ctx)
	retentionDone := make(chan struct{})
	go func() {
		defer close(retentionDone)
		runStorageRetention(retentionCtx, diagnosis, func() retentionScope {
			return retentionScope{store: retained.Store, writes: retained.Writes, limits: retained.Limits, thread: retained.Thread, notice: func(phase, message string) { issues.CleanupNotice("", "", phase, message) }}
		}, func(err error) {
			issues.AddNotice("", "storage_cleanup_failure", fmt.Sprintf("Mekugi could not complete background storage cleanup: %v. Session requests continue unless storage reaches its limit.", err))
		})
	}()
	defer func() {
		stopRetention()
		<-retentionDone
	}()

	stopFailedStartup := func(err error) (int, error) {
		cancel()
		return 1, err
	}

	if len(config.ServiceTiers) != 0 {
		var entries []string
		for _, model := range slices.Sorted(maps.Keys(config.ServiceTiers)) {
			key, err := json.Marshal(model)
			if err != nil {
				return 1, fmt.Errorf("invalid model ID in service tiers: %w", err)
			}
			entries = append(entries, string(key)+"="+strconv.Quote(config.ServiceTiers[model]))
		}
		args = append([]string{"-c", "service_tiers={" + strings.Join(entries, ",") + "}"}, args...)
	}
	cmd := exec.CommandContext(ctx, executable, nativeAppServerArgs(args)...)
	if summary, ok := options.ChildReplySummaryOverride(config.Features); ok {
		cmd.Args = append(cmd.Args, "-c", "features.child_reply_summary="+strconv.FormatBool(summary))
	}
	if threads.headless != nil {
		// Headless automation cannot answer interactive approvals or secret prompts.
		// Append after user CLI overrides so headless runs are always unguarded.
		index := slices.Index(cmd.Args, "--")
		if index < 0 {
			index = len(cmd.Args)
		}
		cmd.Args = slices.Insert(cmd.Args, index, "-c", "features.shell_guard=false")
	}
	self, err := os.Executable()
	if err != nil {
		return stopFailedStartup(err)
	}
	overrides, closeMCP, err := openInvocationMCP(ctx, threads.tools, self)
	defer func() { runErr = errors.Join(runErr, closeMCP()) }()
	if err != nil {
		return stopFailedStartup(err)
	}
	cmd.Args = append(cmd.Args, overrides...)
	cmd.Dir = workspace
	cmd.Env = codexEnvironmentCapability(environment)
	if autoTitle {
		cmd.Env = append(cmd.Env, "MEKUGI_AUTO_TITLE=1")
	}
	if threads.sourceSocket != "" {
		cmd.Env = append(cmd.Env, sessionapi.NativeSkillsSocketEnvironment+"="+threads.sourceSocket)
	}
	if nativeTrace != nil {
		cmd.Env = append(cmd.Env, "CODEX_ROLLOUT_TRACE_ROOT="+nativeTrace.Directory())
	}
	if debug != nil {
		cmd.Env = append(cmd.Env, evidence.AXReadOutputEnvironment+"="+debug.ReadLogPath())
	}
	helper := execTrackHelper()
	var reportEndpoints segmenttransport.Endpoints
	if segments.Live != nil {
		reportEndpoints = segments.Live.Endpoints()
	}
	cmd.Env, err = frontendShellEnvironment(cmd.Env, frontendDirectory, helper, reportEndpoints)
	if err != nil {
		return stopFailedStartup(err)
	}
	var trackingCommand string
	if helper != "" {
		tracker := ""
		for _, entry := range cmd.Env {
			if value, ok := strings.CutPrefix(entry, shellinstrumentation.ShTrackerEnvironment+"="); ok {
				tracker = value
			}
		}
		if tracker != "" {
			trackingCommand = shellhook.TrackingCommand(helper, tracker)
		}
	}

	cmd.Cancel = func() error { return cmd.Process.Signal(syscall.SIGTERM) }
	cmd.WaitDelay = 5 * time.Second
	var waitCodex func() error
	err = ctx.Err()
	if err == nil {
		reportObserver = openShellObservation(segments, issues)
		windows.Reports = reportObserver

		debugDirectory := ""
		if debug != nil {
			debugDirectory = filepath.Dir(debug.Paths()[0])
		}
		waitCodex, err = (&nativeSession{preparer: mekugiCalls, reviewSource: changes, presentationState: views, conversationState: conversation, activity: activity, history: service, segments: segments.Reports, ctx: ctx, controlObservations: &control, usage: usage, audience: audience, review: review, execWindows: windows, nativeTrace: nativeTrace, retained: retained, changeSource: changeSource, snapshots: checkpoints, publishChanges: review.Publish, captureNotice: issues.AddThreadNotice, execTrack: reportObserver, issues: issues, sessionCapture: capture, resumeArgv: resumeArgv, rewindExecutable: filepath.Join(frontendDirectory, "mchanges"), trackingCommand: trackingCommand, replayDebugDirectory: debugDirectory, observations: observations, approvalMode: approvals}).start(prefs, cmd, skillsManagerAvailable, resumeThread, predictionProvider, api, options, threads)

	}
	if err != nil {
		return stopFailedStartup(fmt.Errorf("launch codex: %w", err))
	}
	codexErr := waitCodex()
	if exitErr, ok := errors.AsType[*exec.ExitError](codexErr); ok {
		// UI failures carry context (for example a lost app-server)
		// around the process status. Do not discard that diagnostic merely
		// because we can preserve the child's exit code.
		if status, ok := exitErr.Sys().(syscall.WaitStatus); ok && status.Signaled() {
			return 128 + int(status.Signal()), codexErr
		}
		return exitErr.ExitCode(), codexErr
	}
	if codexErr != nil {
		return 1, codexErr
	}
	return 0, nil
}

// openShellObservation attaches the auxiliary segment-report observer.
func openShellObservation(segments *segmentservice.Service, issues *requestexecution.Failures) *segmentservice.Observer {
	var reportObserver *segmentservice.Observer
	reportErr := segments.LiveError
	if segments.Live != nil {
		var hub *segmentservice.Observer
		hub, reportErr = segmentservice.NewObserver(segments.Live, activitymodel.OutputBytes)
		if reportErr == nil {
			reportObserver = hub
		} else {
			segments.Live.Close()
		}
	}
	if reportErr != nil {
		issues.AddNotice("", "command_reporting_unavailable", "Command segment tracking is unavailable. Commands retain their original output and exit status: "+reportErr.Error())
	}
	return reportObserver
}
