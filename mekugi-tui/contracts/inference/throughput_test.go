package inference

import (
	"testing"
	"time"
)

func TestOutputThroughputAddWeightsByMeasuredDuration(t *testing.T) {
	var total OutputThroughput
	total.Add(OutputThroughput{OutputTokens: 100, DurationNanos: uint64(time.Second), MeasuredRequests: 1})
	total.Add(OutputThroughput{OutputTokens: 90, DurationNanos: uint64(9 * time.Second), MeasuredRequests: 1})
	total.Add(OutputThroughput{})
	want := OutputThroughput{OutputTokens: 190, DurationNanos: uint64(10 * time.Second), MeasuredRequests: 2}
	if total != want {
		t.Fatalf("aggregate = %+v, want %+v", total, want)
	}
}
