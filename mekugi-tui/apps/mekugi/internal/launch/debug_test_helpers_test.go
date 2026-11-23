package launch

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"
	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

func diagnosticDebugOutput(t *testing.T) *requestexecution.Debug {
	t.Helper()
	d, err := openDebugOutput(testDiagnostics(new(evidence.Service), new(transportcapture.Service)), true, "", os.Getenv(evidence.AXReadOutputEnvironment))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = d.Close(); _ = os.RemoveAll(filepath.Dir(d.Paths()[0])) })
	return d
}
