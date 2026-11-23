package launch

import (
	"context"
	"testing"

	history "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay/playback"
	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

func replayTestService(t *testing.T, diagnostics *transportcapture.Service) *playback.Service {
	t.Helper()
	deps := sessionreplay.Deps{History: testHistory()}
	if diagnostics != nil {
		deps.Timing = diagnostics.ReplayTiming
	}
	service, err := sessionreplay.Module.New(deps, module.Slots{})
	if err != nil {
		t.Fatal(err)
	}
	if err := sessionreplay.Module.Bindings.Register(new(binding.Registry), service, nil); err != nil {
		t.Fatal(err)
	}
	return service
}
func replayReadTest(t *testing.T, diagnostics *transportcapture.Service, ctx context.Context, path, debug string, seed uint64) (*playback.Source, error) {
	t.Helper()
	directory, _ := history.ReplayDirectory()
	return replayTestService(t, diagnostics).Read(ctx, playback.ReadOptions{Path: path, HistoryDirectory: directory, DebugDirectory: debug, Seed: seed})
}
