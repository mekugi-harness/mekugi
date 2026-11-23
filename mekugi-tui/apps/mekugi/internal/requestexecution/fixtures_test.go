package requestexecution

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/inferencegateway"
	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"
)

func testCriticalErrors() *Failures { return NewFailures(new(notice.Service)) }
func startedWriter(started bool) *inferencegateway.ResponseWriter {
	w := &inferencegateway.ResponseWriter{ResponseWriter: httptest.NewRecorder()}
	if started {
		w.WriteHeader(http.StatusOK)
	}
	return w
}

type finalAnswerErrorReader struct{ err error }

func (r finalAnswerErrorReader) Read([]byte) (int, error) { return 0, r.err }
func modelTestRequest(t *testing.T, model string) codexresponses.Request {
	t.Helper()
	request, err := codexresponses.ParseRequest(responses.MustMarshalJSON(map[string]any{"model": model, "reasoning": map[string]string{"effort": "medium"}, "input": []any{map[string]string{"role": "user", "content": "keep exact history"}}}))
	if err != nil {
		t.Fatal(err)
	}
	return request
}
