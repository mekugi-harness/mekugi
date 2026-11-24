package capture

import (
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
)

func TestMetricsSnapshotCloneIsolatesPublishedDiagnostics(t *testing.T) {
	source := MetricsSnapshot{Exchanges: []exchangeMetrics{{CacheDiagnosis: &cacheDiagnosis{
		Client:    prefixComparison{ChangedFields: []string{"client"}},
		Projected: prefixComparison{ChangedFields: []string{"projected"}},
		Provider:  prefixComparison{ChangedFields: []string{"provider"}},
	}}}}
	source.StorageWrites = new(persistence.Snapshot)
	source.StorageWrites.Bytes = 7
	clone := source.Clone()
	clone.StorageWrites.Bytes = 0
	clone.Exchanges[0].CacheDiagnosis.Client.ChangedFields[0] = "mutated"
	clone.Exchanges[0].CacheDiagnosis.Projected.ChangedFields[0] = "mutated"
	clone.Exchanges[0].CacheDiagnosis.Provider.ChangedFields[0] = "mutated"
	d := source.Exchanges[0].CacheDiagnosis
	if source.StorageWrites.Bytes != 7 || d.Client.ChangedFields[0] != "client" || d.Projected.ChangedFields[0] != "projected" || d.Provider.ChangedFields[0] != "provider" {
		t.Fatal("clone aliases published diagnostics")
	}
	if got := (MetricsSnapshot{}).Clone(); got.Exchanges != nil {
		t.Fatal("clone changed an absent collection")
	}
}
