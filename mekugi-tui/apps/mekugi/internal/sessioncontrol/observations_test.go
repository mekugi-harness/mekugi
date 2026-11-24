package sessioncontrol

import (
	"errors"
	"testing"
)

func TestApprovalFeedbackFollowsSuccessfulDelivery(t *testing.T) {
	var observations Observations
	failure := errors.New("answer rejected")
	if err := observations.AnswerWithFeedback("main", "turn", "undelivered", func() error { return failure }); !errors.Is(err, failure) {
		t.Fatalf("delivery error = %v", err)
	}
	if observations.Feedback("main", "turn") != "" {
		t.Fatal("failed answer published feedback")
	}
	if err := observations.AnswerWithFeedback("main", "turn", "reason", func() error { return nil }); err != nil {
		t.Fatal(err)
	}
	if observations.Feedback("main", "turn") != "reason" || observations.Feedback("child", "turn") != "" {
		t.Fatal("feedback was lost or escaped its thread")
	}
	observations.ClearFeedback("main", "other")
	if observations.Feedback("main", "turn") != "reason" {
		t.Fatal("unrelated turn removed feedback")
	}
	observations.ClearFeedback("main", "turn")
	if observations.Feedback("main", "turn") != "" {
		t.Fatal("completed turn retained feedback")
	}
}

func TestContextIdentityTracksHistoryReplacement(t *testing.T) {
	var observed Observations
	initial := observed.ContextIdentity("root")
	if initial == "" || observed.ContextIdentity("root") != initial || observed.ContextIdentity("child") == initial {
		t.Fatal("context identity is unstable or shared between threads")
	}
	observed.RefreshContext("root")
	refreshed := observed.ContextIdentity("root")
	if refreshed == initial {
		t.Fatal("rewind or reset reused the old history identity")
	}
	observed.ObserveCompactionContext("root", "window")
	compacted := observed.ContextIdentity("root")
	observed.ObserveCompactionContext("root", "window")
	if compacted == refreshed || compacted != observed.ContextIdentity("root") {
		t.Fatal("native window did not invalidate exactly once")
	}
	if new(Observations).ContextIdentity("root") == initial || (*Observations)(nil).ContextIdentity("root") != "" {
		t.Fatal("cold or missing owner reused a context")
	}
}
