package reports

import (
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

type Service struct{ history *history.Service }

func New(history *history.Service) (*Service, error) {
	if history == nil {
		return nil, errors.New("command segments requires session history")
	}
	return &Service{history: history}, nil
}

// Scope is selected by the caller, never inferred from the process directory.
// Store is the existing publication handle, including its revision-aware size cache.
// An empty thread preserves uncatalogued publication when host identity is unknown.
type Scope struct {
	Store             *history.Store
	Workspace, Thread string
	Limits            history.Limits
	Writes            *persistence.Counter
}

func RecordKinds() []recordkind.Spec {
	return []recordkind.Spec{{ID: "commandsegments", MaxRecordBytes: history.MaxRecordBytes, Match: recordkind.HashedName("segments-", ".json")}}
}

// Inspection selects an actual retained envelope. Legacy is present only for
// historical-only evidence; Data is the canonical new envelope otherwise.
// Dependencies describes the selected envelope, not the union of both forms.
type Inspection struct {
	Name         string
	Data         []byte
	Legacy       *history.Record
	Dependencies []string
}
