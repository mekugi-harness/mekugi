package sessionhistory

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/legacycall"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

type Deps struct {
	// RecordKinds are invocation-owned native receipts supplied by the
	// application. Feature records enter through descriptor contributions.
	RecordKinds []recordkind.Spec
}

var Module = module.Descriptor[Deps, *history.Service, module.NoBindings, module.NoViews]{
	ID:           "sessionhistory",
	Summary:      "Own retained session publication, namespaces, allocation, leases, and retention",
	Lifetime:     module.Invocation,
	HostsRecords: true,
	// Host facts retain their historical codecs and identities. Feature records
	// enter through the same contribution slot as these built-in host records.
	Contributes: module.Contributions[*history.Service]{RecordKinds: []recordkind.Spec{
		history.HookRecord,
		history.CompactionRecord,
		history.CompactSummaryRecord,
		history.FrozenFileRecord,
		history.FrozenResourceRecord,
		history.LegacyOutputRecord,
		history.LegacyCursorRecord,
		{ID: "call", Match: func(name string) bool { return legacycall.ValidName(name, false) }, Dependencies: func(record recordkind.Record) ([]string, error) {
			return history.HostDependencies(record.Name, record.Data)
		}},
		{ID: "snapshot", Match: legacycall.ValidSnapshotName},
		{ID: "commentary", Match: func(name string) bool { return legacycall.ValidName(name, true) }, SeparateBudget: true, Limit: 16 << 20},
	}},
	New: newService,
}
