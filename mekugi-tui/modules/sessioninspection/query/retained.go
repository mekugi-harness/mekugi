package query

import (
	json "encoding/json/v2"

	sessionhistory "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

// Inspection consumes presence, counts and original patch inputs, not endpoint
// or diff text. Keep this projection separate from evidence materialization.
func (s *Service) readCall(directory, workspace, call string) (sessionhistory.Record, bool, error) {
	store := s.history.Attach(directory)
	record, found, err := store.ReadAttachedHostCall(workspace, call, false)
	if err != nil || !found {
		return record.Record, found, err
	}
	root, err := evidence.Root(record)
	if err != nil || root == "" {
		return record.Record, found, err
	}
	source := &evidence.Source{Store: store}
	manifest, err := evidence.Open(source, root)
	if err != nil {
		return record.Record, false, err
	}
	if len(manifest.Observation) > 0 {
		record.History.ExecObservation = new(sessionhistory.Observation)
		if err := json.Unmarshal(manifest.Observation, record.History.ExecObservation); err != nil {
			return record.Record, false, err
		}
	}
	for _, patch := range manifest.Patches {
		record.History.NativePatches = append(record.History.NativePatches, sessionhistory.NativePatch{Input: patch.Input})
	}
	// The inspection projection uses only len(ReviewFiles).
	reviews, err := evidence.ReadReviews(source, manifest.Reviews)
	if err != nil {
		return record.Record, false, err
	}
	record.History.ReviewFiles = make([]sessionhistory.ReviewFile, len(reviews))
	return record.Record, true, nil
}
