package codexbinding

import (
	"encoding/json/v2"
	"reflect"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

func TestInputPreservesPartRelativeBindingsAndSnapshot(t *testing.T) {
	var e composer.Editor
	e.InsertImage("image.png")
	e.InsertText(" @code")
	e.Files = []composer.Token{{Start: 10, End: 15, Path: "code"}}
	e.Attachments = []string{"snapshot"}
	data, err := json.Marshal(Input(e.Draft))
	if err != nil {
		t.Fatal(err)
	}
	want := `[{"path":"image.png","type":"localImage"},{"text":" @code","text_elements":[{"byteRange":{"start":1,"end":6},"placeholder":"@code"}],"type":"text"}]`
	// Decode both objects so map order is irrelevant while field spelling and null/empty stay exact.
	var gotValue, wantValue any
	if err := json.Unmarshal(data, &gotValue); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal([]byte(want), &wantValue); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(gotValue, wantValue) {
		t.Fatalf("wire: %s", data)
	}
}

func TestInputAppendsUserSelectionsAsMarkdown(t *testing.T) {
	const original = "Why is this [Selected message 1] blah blah blah while [Selected diff 1] baz baz baz"
	message := strings.Index(original, "[Selected message 1]")
	diff := strings.Index(original, "[Selected diff 1]")
	draft := composer.Draft{Text: original, Selections: []composer.Token{
		{Start: message, End: message + len("[Selected message 1]"), Text: "foo"},
		{Start: diff, End: diff + len("[Selected diff 1]"), Text: "bar", Source: "source.go"},
	}}
	input := Input(draft)
	var text strings.Builder
	for _, part := range input {
		if value, ok := part["text"].(string); ok {
			text.WriteString(value)
		}
	}
	want := original + "\n\n---\nUser selections:\n- message 1: foo\n- diff 1:\n  ```go\n  bar\n  ```"
	if text.String() != want {
		t.Fatalf("user input = %q, want %q", text.String(), want)
	}
	if draft.Text != original || len(draft.Attachments) != 0 {
		t.Fatal("mapping mutated the draft or created an envelope")
	}
}

func TestInputSelectionFencesAndUploadedBytes(t *testing.T) {
	draft := composer.Draft{Text: "[Selected diff] [Uploaded notes.txt]", Selections: []composer.Token{{Start: 0, End: len("[Selected diff]"), Text: "first\r\n```\nlast"}}, Files: []composer.Token{composer.UploadAttachment("notes.txt", []byte("uploaded text"))}}
	input := Input(draft)
	text := input[len(input)-1]["text"].(string)
	if !strings.Contains(text, "  ````\n  first\r\n  ```\n  last\n  ````") || !strings.HasSuffix(text, "- Uploaded notes.txt: uploaded text") {
		t.Fatalf("plain supplied text lost bytes or fence boundaries: %q", text)
	}
}
