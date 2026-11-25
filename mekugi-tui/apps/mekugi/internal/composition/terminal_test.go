package composition

import (
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

func TestNativeRendererCollectsSelectedViews(t *testing.T) {
	views, err := NativeRenderer(true)
	if err != nil {
		t.Fatalf("NativeRenderer(true): %v", err)
	}
	if !views.ChildReplySummary {
		t.Fatal("ChildReplySummary = false, want true")
	}
	attachment, _ := views.NewAttachment(diffrender.EnvironmentTheme(""))
	if attachment == nil {
		t.Fatal("NewAttachment returned nil")
	}
}
