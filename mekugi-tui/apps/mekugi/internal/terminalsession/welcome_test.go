package terminalsession

import (
	"strings"
	"testing"
)

func TestWelcomeTrimsPrefixForUserAgent(t *testing.T) {
	ua := "mekugi/0.172.0 (Debian 13.0.0; x86_64) ghostty (mekugi; be95b4a4)"
	got := welcome("be95b4a4", ua)
	if strings.Contains(got, "codex v") || !strings.Contains(got, ua) {
		t.Fatalf("expected raw user-agent header without prefix, got %q", got)
	}
	legacy := welcome("dev", "0.158.0")
	if !strings.Contains(legacy, "Mekugi dev • codex v0.158.0") {
		t.Fatalf("expected legacy short version header preserved, got %q", legacy)
	}
}
