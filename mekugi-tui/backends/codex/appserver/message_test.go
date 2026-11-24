package appserver_test

import (
	"bytes"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
)

func TestAppServerMessageKeepsEmptyObjectParams(t *testing.T) {
	for _, params := range []jsontext.Value{nil, []byte(`{}`), []byte(`{"includeHidden":true}`)} {
		wire, err := json.Marshal(&appserver.Message{Method: "model/list", Params: params})
		if err != nil {
			t.Fatal(err)
		}
		var message map[string]jsontext.Value
		if err := json.Unmarshal(wire, &message); err != nil {
			t.Fatal(err)
		}
		if string(message["params"]) != string(params) {
			t.Fatalf("params=%s wire=%s", params, wire)
		}
		if _, present := message["result"]; present {
			t.Fatalf("notification includes result: %s", wire)
		}
	}
}

type rpcWriteBuffer struct{ bytes.Buffer }

func (*rpcWriteBuffer) Close() error { return nil }

func TestAppServerRespondKeepsEmptyResult(t *testing.T) {
	for _, tc := range []struct {
		name   string
		result any
		want   string
	}{
		{"empty object", &struct{}{}, `{}`},
		{"array", []string{}, `[]`},
		{"string", "", `""`},
		{"null", nil, `null`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var wire rpcWriteBuffer
			client := appserver.Client{Input: &wire}
			if err := client.Respond(jsontext.Value("42"), tc.result); err != nil {
				t.Fatal(err)
			}
			want := `{"id":42,"result":` + tc.want + "}\n"
			if got := wire.String(); got != want {
				t.Fatalf("response = %q, want %q", got, want)
			}
		})
	}
}
