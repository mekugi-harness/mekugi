package executionobservation

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

const maxExecIntervals = 64

type execIntervalCut struct {
	writer    string
	workspace string
	number    int
	codeMode  bool
	scope     changes.Scope
}

type execInterval struct {
	call, resume string
	before       mekugiHistory
	after        *changecapture.Observation
}

func execIntervalID(call string, codeMode bool, number int) string {
	return fmt.Sprintf("%s:interval:%d", history.ExecCallID(call, codeMode), number)
}

// Intervals are immutable observations under the original writer's correlation.
// Membership is deferred until that writer's real terminal result. Their earlier
// capture order still places the prefix before the independently completed edit.
func readExecIntervals(ctx context.Context, selected changes.Scope, source changes.Service, workspace, call string, codeMode bool) ([]execInterval, error) {
	if selected.Store == nil {
		return nil, nil
	}
	index, err := source.Inspect(selected, workspace)
	if err != nil {
		return nil, err
	}
	expected := make(map[string]string)
	for changeID, change := range index.Changes {
		if change.Correlation != call+"\x00exec" {
			continue
		}
		for _, id := range change.Dependencies {
			if !strings.HasSuffix(id, ":resume") {
				expected[id] = changeID
			}
		}
	}
	if len(expected) > maxExecIntervals {
		return nil, errors.New("named capture interval limit exceeded")
	}
	var intervals []execInterval
	bytes := 0
	for n := range len(expected) {
		id := execIntervalID(call, codeMode, n)
		if expected[id] == "" {
			return nil, errors.New("invalid named capture interval membership")
		}
		before, found, err := lookupObservation(ctx, selected, workspace, id)
		if err != nil {
			return nil, err
		}
		if !found {
			return nil, errors.New("named capture interval missing")
		}
		if before.CorrelationID != call+"\x00exec" || before.ExecObservation == nil || before.ChangeID != expected[id] {
			return nil, errors.New("invalid named capture interval")
		}
		resume := id + ":resume"
		after, resumed, err := lookupObservation(ctx, selected, workspace, resume)
		if err != nil {
			return nil, err
		}
		if resumed && (after.CorrelationID != before.CorrelationID || after.ChangeID != before.ChangeID || after.ExecObservation == nil) {
			return nil, errors.New("invalid named capture continuation")
		}
		value := execInterval{call: id, resume: resume, before: before}
		if resumed {
			value.after = after.ExecObservation
		}
		for _, file := range before.reviews() {
			bytes += len(file.Diff)
		}
		for _, observation := range []*changecapture.Observation{value.before.ExecObservation, value.after} {
			if observation != nil {
				for _, file := range observation.Files {
					bytes += len(file.Content)
				}
			}
		}
		if bytes > changes.MaxReadBytes {
			return nil, errors.New("named capture intervals exceed retained read bound")
		}
		intervals = append(intervals, value)
	}
	return intervals, nil
}

func rebaseExecIntervals(observation *changecapture.Observation, intervals []execInterval) *changecapture.Observation {
	copy := *observation
	copy.Files = slices.Clone(observation.Files)
	for _, interval := range intervals {
		copy.Excluded = append(slices.Clone(copy.Excluded), interval.before.ExecObservation.Excluded...)
		for _, boundary := range interval.before.ExecObservation.Files {
			// Checkpoint-backed paths become named only at this boundary. Their
			// retained continuation, not the old tree, owns subsequent comparison.
			if !slices.ContainsFunc(copy.Files, func(file changecapture.File) bool { return file.Path == boundary.Path }) {
				copy.Files = append(copy.Files, boundary)
			}
			for i, file := range copy.Files {
				if file.Path != boundary.Path {
					continue
				}
				next := boundary
				next.Error = "overlapping writer endpoint unavailable"
				if interval.after != nil {
					for _, endpoint := range interval.after.Files {
						if endpoint.Path == file.Path {
							next = endpoint
						}
					}
				}
				next.Origin, next.AlsoManaged = file.Origin, file.AlsoManaged
				copy.Files[i] = next
			}
		}
	}
	if len(intervals) == maxExecIntervals {
		for i := range copy.Files {
			copy.Files[i].Error = "named capture interval limit exceeded"
		}
	}
	return &copy
}

func (w *Windows) forgetIntervalClaims(cuts []execIntervalCut, endpoints []changecapture.File) {
	if w == nil || len(cuts) == 0 {
		return
	}
	w.state().mu.Lock()
	defer w.state().mu.Unlock()
	for _, cut := range cuts {
		if window := w.find(cut.writer); window != nil {
			window.claims.Forget(endpoints)
		}
	}
}

type execIntervalContinuation struct {
	scope           changes.Scope
	workspace, call string
	value           mekugiHistory
}

func prepareExecIntervalContinuations(ctx context.Context, workspace string, cuts []execIntervalCut, endpoints []changecapture.File) ([]execIntervalContinuation, error) {
	if len(endpoints) == 0 {
		return nil, nil
	}
	var continuations []execIntervalContinuation
	for _, cut := range cuts {
		continuation, err := prepareExecIntervalContinuation(ctx, workspace, cut, endpoints)
		if err != nil {
			return nil, err
		}
		if continuation != nil {
			continuations = append(continuations, *continuation)
		}
	}
	return continuations, nil
}

func prepareExecIntervalContinuation(ctx context.Context, workspace string, cut execIntervalCut, endpoints []changecapture.File) (*execIntervalContinuation, error) {
	workspace = cmp.Or(cut.workspace, workspace)
	id := execIntervalID(cut.writer, cut.codeMode, cut.number)
	prefix, found, err := lookupObservation(ctx, cut.scope, workspace, id)
	if err != nil {
		return nil, err
	}
	if !found || prefix.ExecObservation == nil {
		return nil, errors.New("named capture boundary missing")
	}
	if _, resumed, err := lookupObservation(ctx, cut.scope, workspace, id+":resume"); err != nil {
		return nil, err
	} else if resumed {
		return nil, nil
	}
	var files []changecapture.File
	budget := changecapture.MaxContentBytes
	for _, boundary := range prefix.ExecObservation.Files {
		for _, endpoint := range endpoints {
			if boundary.Path == endpoint.Path {
				if len(endpoint.Content) > budget {
					endpoint.Content, endpoint.Error = "", "overlapping writer endpoint exceeds capture bound"
				} else {
					budget -= len(endpoint.Content)
				}
				files = append(files, endpoint)
			}
		}
	}
	resume := mekugiHistory{legacy: prefix.legacy, ToolName: nativeExecCommandToolName, Root: workspace, ExecutingThread: prefix.ExecutingThread,
		ChangeID: prefix.ChangeID, CorrelationID: prefix.CorrelationID, Attempt: prefix.Attempt,
		ExecObservation: &changecapture.Observation{Files: files, CodeMode: cut.codeMode}}
	return &execIntervalContinuation{scope: cut.scope, workspace: workspace, call: id + ":resume", value: resume}, nil
}

func (c execIntervalContinuation) publish(tx *history.Transaction, source changes.Service) error {
	if err := publishCapturedObservation(tx, c.scope, c.workspace, c.call, c.value); err != nil {
		return err
	}
	_, err := source.Publish(tx, c.scope, c.workspace, map[string]changes.Publication{c.call: {
		ChangeID: c.value.ChangeID, CorrelationID: c.value.CorrelationID, Dependency: true,
	}})
	return err
}
