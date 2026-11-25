package sessioncontrol

import (
	"encoding/json/jsontext"
	"testing"
)

type fakeTransport struct {
	next     int
	methods  []string
	notified []string
	replied  []string
}

func (f *fakeTransport) Initialize() (string, error) {
	f.next++
	f.methods = append(f.methods, "initialize")
	return "init-1", nil
}

func (f *fakeTransport) Send(method string, _ any, request bool) (string, error) {
	if !request {
		f.notified = append(f.notified, method)
		return "", nil
	}
	f.next++
	f.methods = append(f.methods, method)
	return method + "-1", nil
}

func (f *fakeTransport) Respond(id jsontext.Value, _ any) error {
	f.replied = append(f.replied, string(id))
	return nil
}

func TestRequestsLedger(t *testing.T) {
	tr := &fakeTransport{}
	reqs := NewRequests[string](tr)
	initID, err := reqs.Initialize("init")
	if err != nil || initID != "init-1" {
		t.Fatalf("Initialize() = %q, %v", initID, err)
	}
	if req, ok := reqs.Lookup(initID); !ok || req.Owner != "init" {
		t.Fatalf("Lookup(%q) = %+v, %v", initID, req, ok)
	}
	if req, ok := reqs.Take(initID); !ok || req.Owner != "init" {
		t.Fatalf("Take(%q) = %+v, %v", initID, req, ok)
	}
	if _, ok := reqs.Take(initID); ok {
		t.Fatal("second Take succeeded")
	}
	if err := reqs.Initialized(true); err != nil {
		t.Fatal(err)
	}
	if len(tr.notified) != 1 || tr.notified[0] != "initialized" {
		t.Fatalf("notified = %v", tr.notified)
	}
}
