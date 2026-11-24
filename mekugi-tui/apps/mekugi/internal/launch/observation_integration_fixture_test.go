package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/legacycall"
	changepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/executionobservation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/nativetrace"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	previewbinding "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/codexbinding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
)

// observationFixture owns only integration-test setup. Each request enters the
// production preparer through its public interface and the same borrowed owners.
type observationFixture struct {
	registry *frontends.Registry

	usage          *accounting.Service
	review         *sessionreview.Review
	activity       *sessioncontrol.Audience
	execWindows    *executionobservation.Windows
	nativeTrace    *nativetrace.Reader
	control        *sessioncontrol.Observations
	previewTiming  previewbinding.Timing
	calls          *history.CallCache
	retained       changes.Scope
	changeSource   changes.Service
	snapshots      *changecapture.Workspaces
	publishChanges func([]changes.Update)
	noticeSink     func(string, string, string, string)
}

func (p *observationFixture) preparer() *executionobservation.Preparer {
	return executionobservation.New(executionobservation.Config{
		Review:         p.review,
		Audience:       p.activity,
		Windows:        p.execWindows,
		NativeTrace:    p.nativeTrace,
		Retained:       p.retained,
		Changes:        p.changeSource,
		Checkpoints:    p.snapshots,
		PublishChanges: p.publishChanges,
		Notice:         p.noticeSink,
	})
}
func (p *observationFixture) notice(session, thread, category, message string) {
	if p != nil && p.noticeSink != nil {
		p.noticeSink(session, thread, category, message)
	}
}

const (
	applyPatchToolName        = "apply_patch"
	nativeExecCommandToolName = "exec_command"
	codeModeCarrierCustom     = "custom"
	codeModeCarrierFunction   = "function"
)

func retainedReviewFiles(files []diffmodel.ReviewFile) []legacycall.ReviewFile {
	result := make([]legacycall.ReviewFile, len(files))
	for i, f := range files {
		result[i] = legacycall.ReviewFile(f)
	}
	return result
}
func reviewFiles(files []legacycall.ReviewFile) []diffmodel.ReviewFile {
	result := make([]diffmodel.ReviewFile, len(files))
	for i, f := range files {
		result[i] = diffmodel.ReviewFile(f)
	}
	return result
}
func receiptEvidence(h legacycall.History) changepresentation.ReceiptEvidence {
	result := changepresentation.ReceiptEvidence{Files: reviewFiles(h.ReviewFiles), Source: binding.EditReceiptSource(h.ToolName, h.Source), ChangeID: h.ChangeID}
	if h.ExecOutcome != nil {
		result.Labels = h.ExecOutcome.Labels
		result.DeletedDirectories = h.ExecOutcome.DeletedDirectories
	}
	return result
}
