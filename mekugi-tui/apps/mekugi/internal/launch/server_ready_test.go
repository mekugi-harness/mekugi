package launch

import (
	"context"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"

	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"

	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"

	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/command"
	activitymodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"

	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"

	segmentservice "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/segments"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func TestRunSessionDoesNotNotifyOnStartupFailure(t *testing.T) {
	for _, args := range [][]string{{"--unknown-flag"}, {"--stream-idle-timeout", "0"}, {"--listen", "127.0.0.1:0"}, {"--provider-base-url", "https://example.com"}} {
		if err := testRunSession(testHistory(), t.Context(), testCommands(), args, nil, func(invocationFixture) { t.Error("ready called despite startup failure") }, nil); err == nil {
			t.Fatalf("accepted %q", args)
		}
	}
}

func TestRunSessionRejectsAXCaptureAlias(t *testing.T) {
	directory := t.TempDir()
	capture := filepath.Join(directory, "capture.jsonl")
	if err := os.WriteFile(capture, []byte("retained\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv(evidence.AXReadOutputEnvironment, capture)
	err := testRunSession(testHistory(), t.Context(), testCommands(), []string{"--capture-output", capture}, nil, func(invocationFixture) { t.Error("aliased exports reached readiness") }, nil)
	if err == nil {
		t.Fatal("aliased outputs accepted")
	}
	data, err := os.ReadFile(capture)
	if err != nil || string(data) != "retained\n" {
		t.Fatalf("capture truncated: %q %v", data, err)
	}
}

func TestRunSessionRejectsUnusableReplayStorageBeforeReady(t *testing.T) {
	state := t.TempDir()
	t.Setenv("XDG_STATE_HOME", state)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("CODEX_HOME", t.TempDir())
	t.Setenv("MEKUGI_RUNTIME_DIR", t.TempDir())
	if err := os.WriteFile(filepath.Join(state, "mekugi"), []byte("not a directory"), 0600); err != nil {
		t.Fatal(err)
	}
	err := testRunSession(testHistory(), t.Context(), testCommands(), nil, nil, func(invocationFixture) { t.Error("unusable replay storage reached readiness") }, nil)
	if err == nil || !strings.Contains(err.Error(), "initialize replay storage") {
		t.Fatalf("startup error = %v", err)
	}
}

// Existing router fixtures select raw arguments; production passes parsed values.
func testRunSession(service *history.Service, ctx context.Context, commands []command.Contribution, args []string, issues *requestexecution.Failures, ready func(invocationFixture), artifacts func([]string)) error {
	options, err := launchoptions.Parse(args)
	if errors.Is(err, flag.ErrHelp) {
		return nil
	}
	if err != nil {
		return err
	}
	usage := accounting.New()
	// Dependency setup must not consume the runFixtureInvocation cancellation under test.
	captured, err := newTestCapture(context.WithoutCancel(ctx), "")
	if err != nil {
		return err
	}
	defer captured.Stop(context.Background())
	return runFixtureInvocation(terminalsession.Views{}, testPresentation(), new(conversationmodel.Service), previewmodel.New(changecapture.ReadFile), new(activitymodel.Service), service, testPreferences(), &segmentservice.Service{Reports: testReports()}, captured, usage, new(evidence.Service), new(transportcapture.Service), testDiagnostics(new(evidence.Service), new(transportcapture.Service)), ctx, commands, nil, options, issues, ready, artifacts)
}
