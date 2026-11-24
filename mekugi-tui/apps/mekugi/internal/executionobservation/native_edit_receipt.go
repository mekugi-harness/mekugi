package executionobservation

import (
	"context"
	"fmt"

	sessionhistory "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	changepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

// publishEditReceipt reads committed filesystem changes. It never accepts a
// caller-supplied diff or participates in edit execution. Command exit status
// does not change the filesystem effects that this receipt describes.
func publishEditReceipt(ctx context.Context, selected changes.Scope, source changes.Service, workspace, thread, callID string, activity *sessioncontrol.Audience, displayID string) error {
	selected = observationScope(ctx, selected)
	return selected.Store.Transaction(ctx, selected.Writes, func(*sessionhistory.Transaction) error {
		if scope, found, err := selected.Store.ReadScope(thread); err != nil {
			return err
		} else if found {
			selected.Namespace = scope.Namespace
		}
		record, found, err := selected.Store.ReadAttachedHostCall(workspace, callID, false)
		if err != nil {
			return err
		}
		if !found || record.History.ToolName != applyPatchToolName && record.History.ExecOutcome == nil ||
			record.History.ExecutingThread != thread {
			return fmt.Errorf("edit receipt not found")
		}
		index, err := source.Inspect(selected, workspace)
		if err != nil {
			return err
		}
		id := record.History.ChangeID
		for _, call := range index.Changes[id].Calls {
			if call.ID != callID || call.Thread != thread {
				continue
			}
			history := legacyHistory(record.History)
			history.sourceAnchor, err = evidence.Anchor(captureSource(selected), record)
			if err != nil {
				return err
			}
			history.ReviewFiles = nil
			history.reviewFiles = []evidence.Review{}
			for _, member := range index.Changes[id].Calls {
				part, found, err := selected.Store.ReadAttachedHostCall(workspace, member.ID, false)
				if err != nil {
					return err
				}
				if !found {
					return fmt.Errorf("edit receipt interval is unavailable")
				}
				reviews, err := evidence.Reviews(captureSource(selected), part)
				if err != nil {
					return err
				}
				history.reviewFiles = append(history.reviewFiles, reviews...)
			}
			receipt, err := sessionreview.CapturedRetainedReceipt(workspace, record.History, history.reviews())
			if err != nil {
				return err
			}
			if receipt != nil {
				receipt.SourceAnchor = history.sourceAnchor
			}
			if receipt == nil && history.ToolName == applyPatchToolName && displayID != "" {
				receipt = changepresentation.NewReceipt(workspace, receiptEvidence(history))
				receipt.Calls = []string{displayID}
			}
			if receipt != nil && receipt.Text != "" {
				activity.Collect(activitypresentation.Supplement{Thread: thread, Source: "edit-receipt\x00" + workspace + "\x00" + callID, Kind: activitypresentation.SupplementEditReceipt, Text: receipt.Text, Calls: receipt.Calls, SourceAnchor: receipt.SourceAnchor})
			}
			return nil
		}
		return fmt.Errorf("edit receipt does not belong to publishing thread")
	})
}

// receiptEvidence translates observation facts into renderer-neutral receipt input.
func receiptEvidence(value mekugiHistory) changepresentation.ReceiptEvidence {
	files, counts, truncated, err := evidence.ReceiptFiles(value.reviews(), changepresentation.MaxReceiptDiffBytes)
	if err != nil {
		files = []diffmodel.ReviewFile{diffmodel.RenderIncompleteReviewFile("", "", err.Error())}
		counts = nil
		truncated = nil
	}
	evidence := changepresentation.ReceiptEvidence{Files: files, Counts: counts, Truncated: truncated, Source: value.Source, ChangeID: value.ChangeID}
	evidence.Source = binding.EditReceiptSource(value.ToolName, value.Source)
	if value.ExecOutcome != nil {
		evidence.Labels, evidence.DeletedDirectories = value.ExecOutcome.Labels, value.ExecOutcome.DeletedDirectories
	}
	return evidence
}
