package executionobservation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessioninspection/query"
)

type sessionInspection struct {
	query.Result
	AX *evidence.InspectionReport `json:"ax,omitempty"`
}

type sessionCorpus = query.Corpus
