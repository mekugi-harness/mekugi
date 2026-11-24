package usage_test

import (
	"bytes"
	"crypto/sha256"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/moduletest"
)

// Source: internal/router/thread_usage_store.go, request_observation.go,
// output_throughput.go and capturer/throughput.go@47bcdd8e.
// This wire fixture is independent of the current record encoder and types.
const historicalUsageV1 = `{
 "Version":1,"Thread":"historical-thread",
 "Counts":{"TotalsKnown":false,"InputTokens":100,"UncachedInputTokens":70,
 "CacheWriteTokens":5,"OutputTokens":20,"ReasoningTokens":8,
 "Inconsistent":false,"Incomplete":false,"ServiceTier":""},
 "UncachedCost":1.25,"CachedCost":0.5,"OutputCost":2.75,
 "CostKnown":true,"Complete":true,"PriorUnknown":false,
 "MissingUsage":1,"Roundtrips":3,"Models":["historical-model"],
 "RoundOutput":{"StartedUnixNano":123456789,
 "Throughput":{"output_tokens":20,"duration_ns":2000000000,"measured_requests":1}}
}`

func usageRecordName(thread string) string {
	return fmt.Sprintf("usage-%x.json", sha256.Sum256([]byte(thread)))
}

func composedUsage(t *testing.T, directory string, notice func(string, error)) (*accounting.Service, *history.Service) {
	t.Helper()
	owner := moduletest.New(t, sessionhistory.Module, sessionhistory.Deps{}, module.Slots{RecordKinds: usage.Module.Contributes.RecordKinds}, nil)
	service := moduletest.New(t, usage.Module, usage.Deps{}, module.Slots{}, nil)
	service.Configure(owner.Attach(directory), nil, history.Limits{Total: history.DefaultStorageBytes}, notice)
	return service, owner
}

func TestModuleReadsHistoricalUsageWithoutWriting(t *testing.T) {
	directory := t.TempDir()
	path := filepath.Join(directory, usageRecordName("historical-thread"))
	if err := os.WriteFile(path, []byte(historicalUsageV1), 0600); err != nil {
		t.Fatal(err)
	}
	service, owner := composedUsage(t, directory, func(thread string, err error) {
		t.Errorf("reading %q: %v", thread, err)
	})
	kind, found, err := owner.Kind(filepath.Base(path))
	if err != nil || !found || kind.ID != "usage" {
		t.Fatalf("historical record registration: kind=%+v found=%v err=%v", kind, found, err)
	}
	want := accounting.Report{
		Usage: inference.Usage{TotalsKnown: true, InputTokens: 100, UncachedInputTokens: 70, CacheWriteTokens: 5, OutputTokens: 20, ReasoningTokens: 8},
		Cost:  4.5, CostKnown: true, MissingUsage: 1, Roundtrips: 3, PriorUnknown: true,
		OutputThroughput: inference.OutputThroughput{OutputTokens: 20, DurationNanos: 2000000000, MeasuredRequests: 1},
	}
	service.Restore("historical-thread", true)
	if got := service.Snapshot("historical-thread"); got != want {
		t.Fatalf("restored legacy usage: got %+v, want %+v", got, want)
	}
	service.Restore("no-record", true)
	if got := service.Snapshot("no-record"); got != (accounting.Report{PriorUnknown: true}) {
		t.Fatalf("absent legacy usage: %+v", got)
	}
	if got := service.Snapshot("fresh"); got != (accounting.Report{}) {
		t.Fatalf("fresh thread inherited consumption: %+v", got)
	}
	data, err := history.ReadRecord(path)
	if err != nil || !bytes.Equal(data, []byte(historicalUsageV1)) {
		t.Fatalf("legacy usage changed: %s (%v)", data, err)
	}
	if _, err := os.Stat(filepath.Join(directory, usageRecordName("no-record"))); !os.IsNotExist(err) {
		t.Fatalf("restoring missing usage created a legacy record or could not check it: %v", err)
	}
}

func TestModuleRejectsLegacyUsageForAnotherThread(t *testing.T) {
	directory := t.TempDir()
	if err := os.WriteFile(filepath.Join(directory, usageRecordName("wrong-thread")), []byte(historicalUsageV1), 0600); err != nil {
		t.Fatal(err)
	}
	var notices []string
	service, _ := composedUsage(t, directory, func(thread string, _ error) {
		notices = append(notices, thread)
	})
	for range 2 {
		if got := service.Snapshot("wrong-thread"); got != (accounting.Report{PriorUnknown: true}) {
			t.Fatalf("foreign usage admitted: %+v", got)
		}
	}
	if len(notices) != 1 || notices[0] != "wrong-thread" {
		t.Fatalf("historical usage notices: %v", notices)
	}
}
