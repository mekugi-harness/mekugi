package launch

import (
	"context"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/headless"
	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

type headlessRequestKind uint8

const (
	headlessReqInitialize headlessRequestKind = iota + 1
	headlessReqThreadOpen
	headlessReqTurnStart
	headlessReqSubtreeList
)

type headlessRequest struct {
	kind headlessRequestKind
}

type headlessSessionConfig struct {
	routerArgs  []string
	args        []string
	workspace   string
	environment []string
	prompt      string
	output      io.Writer
	allowResume bool
	parsed      *parsedHeadlessArgs
}

type parsedHeadlessArgs struct {
	options      launchoptions.Config
	overrides    []string
	resumeThread string
}

func parseHeadlessArgs(routerArgs, args []string, allowResume bool) (parsedHeadlessArgs, error) {
	options, err := launchoptions.Parse(routerArgs)
	if err != nil {
		return parsedHeadlessArgs{}, err
	}
	profile, cliArgs, err := launchoptions.ProfileSelector(args)
	if err != nil {
		return parsedHeadlessArgs{}, err
	}
	options.Profile = profile
	if options.RequireCustomProvider && options.Profile == "" {
		return parsedHeadlessArgs{}, errors.New("mekugi requires a custom Responses provider profile: configure NAME.config.toml and run mekugi -p NAME; use mekugi codex for native OpenAI")
	}
	overrides, resumeThread, yolo, err := appServerArgs(expandReasoningShortcuts(cliArgs))
	if err != nil {
		return parsedHeadlessArgs{}, err
	}
	if err := validateProviderOverrides(overrides, options.Profile); err != nil {
		return parsedHeadlessArgs{}, err
	}
	if !yolo {
		return parsedHeadlessArgs{}, errors.New("headless cannot answer approvals; it requires explicit --yolo")
	}
	if resumeThread != "" && !allowResume {
		return parsedHeadlessArgs{}, errors.New("headless runs a new thread; resume is not supported")
	}
	return parsedHeadlessArgs{options: options, overrides: overrides, resumeThread: resumeThread}, nil
}

// runHeadless runs a single-prompt noninteractive session directly against
// mekugi-app-server without sessionapi.Hub or a renderer.
func runHeadless(ctx context.Context, routerArgs, args []string, input io.Reader, output io.Writer) (int, error) {
	parsed, err := parseHeadlessArgs(routerArgs, args, false)
	if err != nil {
		return 2, err
	}
	prompt, err := headless.ReadPrompt(ctx, input)
	if err != nil {
		return 1, err
	}
	workspace, err := os.Getwd()
	if err != nil {
		return 1, err
	}
	return runHeadlessSession(ctx, headlessSessionConfig{
		workspace:   workspace,
		environment: invocationEnvironment(ctx),
		prompt:      prompt,
		output:      output,
		parsed:      &parsed,
	})
}

func runHeadlessSession(ctx context.Context, cfg headlessSessionConfig) (code int, runErr error) {
	parsed := cfg.parsed
	if parsed == nil {
		p, err := parseHeadlessArgs(cfg.routerArgs, cfg.args, cfg.allowResume)
		if err != nil {
			return 2, err
		}
		parsed = &p
	}
	options, overrides, resumeThread := parsed.options, parsed.overrides, parsed.resumeThread
	settings, err := launchoptions.LoadSettings()
	if err != nil {
		return 1, err
	}
	options = options.ResolveFeatures(settings.Features)
	workspace, err := filepath.Abs(cfg.workspace)
	if err != nil {
		return 1, fmt.Errorf("resolve invocation workspace: %w", err)
	}
	environment := slices.Clone(cfg.environment)
	if environment == nil {
		environment = invocationEnvironment(ctx)
	}
	var readLogPath string
	for _, entry := range environment {
		if value, ok := strings.CutPrefix(entry, evidence.AXReadOutputEnvironment+"="); ok {
			readLogPath = value
		}
	}
	if err := evidence.ValidateOutputAliases(readLogPath, options.CaptureOutput); err != nil {
		return 1, err
	}
	executable, err := invocationExecutable("mekugi-app-server", environment, workspace)
	if err != nil {
		return 1, fmt.Errorf("locate mekugi-app-server: %w", err)
	}
	environment = serverEnvironment(environment)

	var debug *diagnostic.Bundle
	var writes *persistence.Counter
	if options.Debug {
		rt, rtErr := composition.Frontends(ctx, false, options.ChildReplySummary, "")
		if rtErr != nil {
			return 1, rtErr
		}
		defer func() {
			runErr = errors.Join(runErr, rt.Close(context.WithoutCancel(ctx)))
		}()
		debug, err = openDebugOutput(rt.Diagnostics, true, options.CaptureOutput, readLogPath)
		if err != nil {
			return 1, err
		}
		writes = debug.Writes()
		defer func() {
			debug.Event(map[string]any{"event": "router_stop", "failed": runErr != nil, "storage_writes": debug.Writes().Snapshot()})
			runErr = errors.Join(runErr, debug.Close())
		}()
	}

	capturePath := options.CaptureOutput
	if debug != nil {
		capturePath = debug.CapturePath()
	}
	var capture *transportcapture.Recorder
	var metricsFile *os.File
	if capturePath != "" || debug != nil {
		if capturePath != "" {
			capturePath, err = filepath.Abs(capturePath)
			if err != nil {
				return 1, fmt.Errorf("resolve capture output: %w", err)
			}
		}
		capture, err = new(transportcapture.Service).Open(transportcapture.Config{Output: capturePath, Writes: writes})
		if err != nil {
			return 1, fmt.Errorf("initialize capture: %w", err)
		}
		defer func() {
			runErr = errors.Join(runErr, capture.Close())
			if metricsFile != nil {
				runErr = errors.Join(runErr, capture.WriteMetrics(writes.Writer(metricsFile)), metricsFile.Close())
			}
		}()
		if debug != nil {
			metricsFile, err = os.OpenFile(debug.MetricsPath(), os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600)
			if err != nil {
				return 1, fmt.Errorf("open metrics output: %w", err)
			}
		}
	}

	overrides, err = prependServiceTiers(overrides, settings.ServiceTiers)
	if err != nil {
		return 1, err
	}

	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stream := headless.NewStream(cfg.output, cancel)

	cmd := exec.CommandContext(ctx, executable, nativeAppServerArgs(overrides)...)
	if summary, ok := options.ChildReplySummaryOverride(settings.Features); ok {
		cmd.Args = append(cmd.Args, "-c", "features.child_reply_summary="+strconv.FormatBool(summary))
	}
	// Headless automation cannot answer interactive approvals or secret prompts.
	// Append after user CLI overrides so headless runs are always unguarded.
	index := slices.Index(cmd.Args, "--")
	if index < 0 {
		index = len(cmd.Args)
	}
	cmd.Args = slices.Insert(cmd.Args, index, "-c", "features.shell_guard=false")
	cmd.Dir = workspace
	cmd.Env = codexEnvironmentCapability(environment)
	if debug != nil {
		cmd.Env = append(cmd.Env, evidence.AXReadOutputEnvironment+"="+debug.ReadLogPath())
	}
	cmd.Cancel = func() error { return cmd.Process.Signal(syscall.SIGTERM) }
	cmd.WaitDelay = 5 * time.Second

	thread, err := runHeadlessHost(ctx, cmd, options, resumeThread, cfg.prompt, stream, capture)
	err = errors.Join(err, stream.Err())
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

func runHeadlessHost(ctx context.Context, cmd *exec.Cmd, options launchoptions.Config, resumeThread, prompt string, stream *headless.Stream, capture *transportcapture.Recorder) (rootThread string, runErr error) {
	host, err := appserver.Start(cmd)
	if err != nil {
		return "", fmt.Errorf("launch codex: %w", err)
	}
	requests := sessioncontrol.NewRequests[headlessRequest](host)
	family := &sessioncontrol.ThreadFamily{
		RequireCustomProvider: options.RequireCustomProvider,
		Environment:           make(map[string]string, len(cmd.Env)),
		Profile:               options.Profile,
	}
	for _, entry := range cmd.Environ() {
		if key, value, ok := strings.Cut(entry, "="); ok {
			family.Environment[key] = value
		}
	}
	startupConfig := appServerResumeConfig(cmd.Args, nil)

	exited := false
	defer func() {
		if !exited {
			if runErr != nil || ctx.Err() != nil {
				_ = host.Cancel()
			}
			runErr = errors.Join(runErr, host.Shutdown())
		}
		if runErr != nil {
			if diagnostic := host.Diagnostics.Snapshot(); diagnostic != "" {
				runErr = fmt.Errorf("%w\nCodex: %s", runErr, diagnostic)
			}
			stream.Fail(runErr)
		}
		_ = host.Input.Close()
		_ = host.Output.Close()
	}()

	if _, err := requests.Initialize(headlessRequest{kind: headlessReqInitialize}); err != nil {
		return "", err
	}

	var treePending, treeDirty, rootCompleted bool
	observeTree := func() error {
		if rootThread == "" || treePending || !treeDirty {
			return nil
		}
		_, err := requests.Send("thread/subtree/list", map[string]any{"threadId": rootThread}, "native/tree", headlessRequest{kind: headlessReqSubtreeList})
		if err == nil {
			treePending, treeDirty = true, false
		}
		return err
	}

	pollTick := time.NewTicker(100 * time.Millisecond)
	defer pollTick.Stop()

	for {
		select {
		case <-ctx.Done():
			return rootThread, ctx.Err()
		case <-pollTick.C:
			if rootCompleted && !treePending {
				treeDirty = true
				if err := observeTree(); err != nil {
					return rootThread, headlessWriteErr(ctx, err)
				}
			}
		case message, ok := <-host.Messages:
			if !ok {
				exited = true
				if err := ctx.Err(); err != nil {
					return rootThread, errors.Join(err, <-host.Done)
				}
				return rootThread, errors.Join(errors.New("app-server disconnected; active work may be incomplete"), <-host.Done)
			}
			if err := stream.Observe(message); err != nil {
				return rootThread, err
			}
			if message.Method != "" && len(message.ID) != 0 {
				err := fmt.Errorf("headless cannot answer host request %s; continue interactively", message.Method)
				stream.Fail(err)
				return rootThread, err
			}
			if err := observeHeadlessCapture(capture, message); err != nil {
				return rootThread, err
			}
			if message.Method == "" {
				req, ok := requests.Lookup(string(message.ID))
				if !ok {
					continue
				}
				requests.Take(string(message.ID))
				switch req.Owner.kind {
				case headlessReqInitialize:
					if message.Error != nil {
						return rootThread, fmt.Errorf("initialize: %s", message.Error.Message)
					}
					if err := requests.Initialized(true); err != nil {
						return rootThread, headlessWriteErr(ctx, err)
					}
					if err := openHeadlessThread(requests, cmd.Dir, resumeThread, family, startupConfig); err != nil {
						return rootThread, headlessWriteErr(ctx, err)
					}
				case headlessReqThreadOpen:
					if message.Error != nil {
						return rootThread, fmt.Errorf("%s: %s", req.Method, message.Error.Message)
					}
					var opened struct {
						Thread struct {
							ID string `json:"id"`
						} `json:"thread"`
					}
					if err := json.Unmarshal(message.Result, &opened); err != nil {
						return rootThread, fmt.Errorf("%s: decode thread: %w", req.Method, err)
					}
					if opened.Thread.ID == "" {
						return rootThread, fmt.Errorf("%s: missing thread id", req.Method)
					}
					rootThread = opened.Thread.ID
					params := map[string]any{"threadId": rootThread, "input": appserver.Input(prompt)}
					if _, err := requests.Send("turn/start", params, rootThread, headlessRequest{kind: headlessReqTurnStart}); err != nil {
						return rootThread, headlessWriteErr(ctx, err)
					}
				case headlessReqTurnStart:
					if message.Error != nil {
						return rootThread, fmt.Errorf("turn/start: %s", message.Error.Message)
					}
				case headlessReqSubtreeList:
					treePending = false
					if message.Error != nil {
						treeDirty = false
						continue
					}
					done, rootDone, stateErr := evaluateHeadlessFamily(rootThread, message.Result)
					if stateErr != nil {
						return rootThread, stateErr
					}
					rootCompleted = rootDone
					if done {
						return rootThread, nil
					}
					if err := observeTree(); err != nil {
						return rootThread, headlessWriteErr(ctx, err)
					}
				}
				continue
			}
			switch message.Method {
			case "thread/started", "thread/status/changed", "turn/started", "turn/completed", "thread/settings/updated", "thread/tokenUsage/updated":
				treeDirty = true
				if err := observeTree(); err != nil {
					return rootThread, headlessWriteErr(ctx, err)
				}
			}
		}
	}
}

func openHeadlessThread(requests *sessioncontrol.Requests[headlessRequest], workspace, resumeThread string, family *sessioncontrol.ThreadFamily, startupConfig map[string]any) error {
	if resumeThread != "" {
		config := maps.Clone(startupConfig)
		params := sessioncontrol.ThreadPermissions(map[string]any{"threadId": resumeThread, "config": config, "modelProvider": config["model_provider"]}, false)
		params["family"] = family
		if tier, ok := config["service_tier"]; ok {
			params["serviceTier"] = tier
			delete(config, "service_tier")
		}
		_, err := requests.Send("thread/resume", params, "main", headlessRequest{kind: headlessReqThreadOpen})
		return err
	}
	params := sessioncontrol.ThreadPermissions(map[string]any{"family": family, "cwd": workspace}, false)
	if trust, explicit := startupConfig["bypass_hook_trust"]; explicit {
		params["config"] = map[string]any{"bypass_hook_trust": trust}
	}
	_, err := requests.Send("thread/start", params, "main", headlessRequest{kind: headlessReqThreadOpen})
	return err
}

// evaluateHeadlessFamily checks whether Main has finished and all family members
// have reached a terminal state.
func evaluateHeadlessFamily(rootThread string, raw []byte) (done, rootCompleted bool, err error) {
	var response struct {
		Data []binding.ThreadInfo `json:"data"`
	}
	if json.Unmarshal(raw, &response) != nil {
		return false, false, nil
	}
	var root *binding.ThreadInfo
	activeChildren := false
	for i := range response.Data {
		member := &response.Data[i]
		if member.ID == rootThread {
			root = member
			continue
		}
		switch member.AgentState() {
		case "pending_init", "running":
			activeChildren = true
		}
	}
	if root == nil {
		return false, false, nil
	}
	switch root.AgentState() {
	case "completed":
		return !activeChildren, true, nil
	case "errored", "interrupted", "shutdown":
		return false, false, fmt.Errorf("headless native work: %s", root.AgentState())
	default:
		return false, false, nil
	}
}

func headlessWriteErr(ctx context.Context, err error) error {
	if cause := ctx.Err(); cause != nil {
		if pipe, ok := errors.AsType[*os.PathError](err); ok && pipe.Op == "write" &&
			(errors.Is(pipe.Err, os.ErrClosed) || errors.Is(pipe.Err, syscall.EPIPE)) {
			return cause
		}
	}
	return err
}

func observeHeadlessCapture(capture *transportcapture.Recorder, message appserver.Message) error {
	if capture == nil {
		return nil
	}
	switch message.Method {
	case "rawResponse/completed":
		var event binding.ResponseCompleted
		if err := json.Unmarshal(message.Params, &event); err != nil {
			return err
		}
		if event.ThreadID == "" || event.ResponseID == "" {
			return nil
		}
		var counts inference.Usage
		if u := event.Usage; u != nil {
			counts = inference.Usage{
				TotalsKnown:         true,
				InputTokens:         u.InputTokens,
				UncachedInputTokens: u.InputTokens - min(u.CachedInputTokens, u.InputTokens),
				CacheWriteTokens:    u.CacheWriteInputTokens,
				OutputTokens:        u.OutputTokens,
				ReasoningTokens:     u.ReasoningOutputTokens,
				Incomplete: event.UsageMetadata == nil || event.UsageMetadata.Metadata.Input.Cached == nil ||
					event.UsageMetadata.Metadata.Output.Reasoning == nil,
				Inconsistent: u.CachedInputTokens > u.InputTokens || u.ReasoningOutputTokens > u.OutputTokens,
			}
		}
		capture.ObserveResponse(event.ThreadID, event.TurnID, event.Model, counts, time.Duration(event.RequestDurationMS)*time.Millisecond)
	case "error":
		var event binding.Notification
		if err := json.Unmarshal(message.Params, &event); err != nil {
			return err
		}
		if !event.WillRetry && event.ThreadID != "" && event.Error != nil {
			capture.ObserveFailure(event.ThreadID, event.TurnID)
		}
	}
	return nil
}

func prependServiceTiers(args []string, tiers map[string]string) ([]string, error) {
	if len(tiers) == 0 {
		return args, nil
	}
	var entries []string
	for _, model := range slices.Sorted(maps.Keys(tiers)) {
		key, err := json.Marshal(model)
		if err != nil {
			return nil, fmt.Errorf("invalid model ID in service tiers: %w", err)
		}
		entries = append(entries, string(key)+"="+strconv.Quote(tiers[model]))
	}
	return append([]string{"-c", "service_tiers={" + strings.Join(entries, ",") + "}"}, args...), nil
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
