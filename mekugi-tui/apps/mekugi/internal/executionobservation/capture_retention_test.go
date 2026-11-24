package executionobservation

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func TestBoundedReceiptAdaptersKeepOmissionUncertainty(t *testing.T) {
	store, err := openMekugiReplayStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	file := diffmodel.RenderReviewFile("", "/w/large", "", strings.Repeat("01234567890123456789012345678901234567890123456789\n", 1000))
	for _, chunked := range []bool{false, true} {
		review := evidence.Inline(file)
		if chunked {
			budget := 0
			review = evidence.RetainReview(t.Context(), captureSource(store.changeScope()), file, &budget)
		}
		value := mekugiHistory{ToolName: nativeExecCommandToolName, CorrelationID: "writer\x00exec", ExecOutcome: &changecapture.Outcome{Status: changecapture.StatusCompleted}, reviewFiles: []evidence.Review{review}}
		live := presentation.NewReceipt("/w", receiptEvidence(value))
		retained, err := sessionreview.CapturedRetainedReceipt("/w", retainedHistory(value), value.reviews())
		if err != nil || retained == nil {
			t.Fatalf("retained receipt: %v", err)
		}
		for _, receipt := range []*presentation.Receipt{live, retained} {
			if !strings.Contains(receipt.Text, "… more diff content") || !strings.Contains(receipt.Text, "+1000 -0") {
				t.Fatalf("chunked=%v adapter dropped truncation or totals", chunked)
			}
		}
	}
}

func TestMissingChunkPreservesIndependentReadTargetsAndRejectsMutation(t *testing.T) {
	f := newMChangesSliceFixture(t, "missing-chunk")
	badPath, goodPath := filepath.Join(f.workspace, "bad.txt"), filepath.Join(f.workspace, "good.txt")
	before, after := strings.Repeat("before\n", 10000), strings.Repeat("after\n", 10000)
	writeTestFile(t, badPath, after)
	writeTestFile(t, goodPath, "new\n")
	bad, good := f.reserve(t, f.thread, "bad"), f.reserve(t, f.thread, "good")
	f.publish(t, bad, "bad", "bad-call", mekugiHistory{ReviewFiles: []diffmodel.ReviewFile{diffmodel.RenderReviewFile(badPath, badPath, before, after)}})
	f.publish(t, good, "good", "good-call", mekugiHistory{ReviewFiles: []diffmodel.ReviewFile{diffmodel.RenderReviewFile(goodPath, goodPath, "old\n", "new\n")}})
	record, found, err := f.store.publication.ReadAttachedHostCall(f.workspace, "bad-call", false)
	if err != nil || !found {
		t.Fatalf("retained call: %v %v", found, err)
	}
	reviews, err := evidence.Reviews(&evidence.Source{Store: f.store.publication}, record)
	if err != nil || len(reviews) != 1 || reviews[0].Text == nil {
		t.Fatalf("large review was not retained: %v", err)
	}
	missing := reviews[0].Text.Parts[0]
	if err := os.Remove(filepath.Join(f.store.directory, missing)); err != nil {
		t.Fatal(err)
	}
	for _, view := range []string{"", "--history ", "--summary ", "--list "} {
		out, stderr, status := f.run(t, "mchanges "+view+bad+" "+good)
		wanted, unwanted := "good.txt", "bad.txt"
		if view == "--list " {
			wanted, unwanted = good, bad
		}
		if status != 1 || !strings.Contains(stderr, missing) || !strings.Contains(out, wanted) || strings.Contains(out, unwanted) {
			t.Fatalf("%q lost independent target: %q %q (%d)", view, out, stderr, status)
		}
	}
	_, stderr, status := f.run(t, "mchanges revert "+bad+" "+good)
	if status != 1 || !strings.Contains(stderr, missing) {
		t.Fatalf("missing mutation source accepted: %q (%d)", stderr, status)
	}
	for path, want := range map[string]string{badPath: after, goodPath: "new\n"} {
		got, err := os.ReadFile(path)
		if err != nil || string(got) != want {
			t.Fatalf("rejected mutation changed %s: %v", path, err)
		}
	}
}

func TestCaptureStorageFailureClassificationAndRetryCause(t *testing.T) {
	for _, captured := range []bool{false, true} {
		t.Run(fmt.Sprintf("captured=%v", captured), func(t *testing.T) {
			store, err := openMekugiReplayStore(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			selected := store.changeScope()
			selected.Limits.Total = 1
			value := mekugiHistory{ToolName: "shell", Script: "true"}
			if captured {
				value.ReviewFiles = []diffmodel.ReviewFile{diffmodel.RenderReviewFile("/w/a", "/w/a", "before\n", "after\n")}
			}
			err = publishObservations(t.Context(), selected, store.changes, nil, "/w", map[string]mekugiHistory{"call": value})
			diagnostic, ok := errors.AsType[*requestexecution.DiagnosticError](err)
			if !ok || diagnostic.Code != "storage_capacity" {
				t.Fatalf("storage failure lost classification: %T %v", err, err)
			}
			if _, ok := errors.AsType[*history.CapacityError](err); !ok {
				t.Fatalf("retry cause lost: %v", err)
			}
		})
	}
}
