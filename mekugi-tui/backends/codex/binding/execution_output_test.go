package binding

import "testing"

func TestCodeModeFailureTextRecognizesPagedFailure(t *testing.T) {
	header := "Script failed (more output: wait with cell ID 7)\nWall time 0.1 seconds\nOutput:\n"
	if text, ok := CodeModeFailureText([]string{header, "partial output"}); !ok || text != "exec script failed" {
		t.Fatalf("failure annotation = %q, %v", text, ok)
	}
	if text, ok := CodeModeFailureText([]string{header, "partial output", "Script error:\nTypeError"}); !ok || text != "exec script failed: TypeError" {
		t.Fatalf("failure annotation = %q, %v", text, ok)
	}
}
