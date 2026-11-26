package connector

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
)

func TestParseAndProbeFleet(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/version" && r.Header.Get(sessionapi.ProtocolHeader) == sessionapi.ProtocolVersion {
			w.WriteHeader(http.StatusOK)
			return
		}
		w.WriteHeader(http.StatusConflict)
	}))
	defer srv.Close()

	raw := `{"boxes":[{"name":"netcup","endpoint":"` + srv.URL + `","default":true,"labels":{"arch":"amd64"}},{"name":"bad","endpoint":"grpc://127.0.0.1:50052"}]}`
	nodes, err := ParseFleetJSON([]byte(raw))
	if err != nil {
		t.Fatalf("ParseFleetJSON: %v", err)
	}
	if len(nodes) != 2 {
		t.Fatalf("expected 2 nodes, got %d", len(nodes))
	}
	statuses := ProbeFleet(t.Context(), srv.Client(), nodes)
	if !statuses[0].Reachable || statuses[0].LatencyMs <= 0 {
		t.Fatalf("expected reachable first node with latency, got %+v", statuses[0])
	}
	if statuses[1].Reachable || statuses[1].Error == "" {
		t.Fatalf("expected non-http node to report error, got %+v", statuses[1])
	}
}
