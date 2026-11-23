package launch

import (
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/rollout"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func openDebugOutput(service *diagnostic.Service, enabled bool, capturePath, readLogPath string) (*requestexecution.Debug, error) {
	if !enabled {
		return nil, nil
	}
	if service == nil {
		return nil, errors.New("diagnostics are unavailable")
	}
	root, err := debugStorageDirectory()
	if err != nil {
		return nil, err
	}
	replay, err := history.ReplayDirectory()
	if err != nil {
		return nil, err
	}
	bundle, err := service.OpenBundle(diagnostic.BundleOptions{Root: root, ReplayDirectory: replay, CapturePath: capturePath, ReadLogPath: readLogPath, Discover: rollout.Discover})
	if err != nil {
		return nil, err
	}
	return &requestexecution.Debug{Bundle: bundle}, nil
}
