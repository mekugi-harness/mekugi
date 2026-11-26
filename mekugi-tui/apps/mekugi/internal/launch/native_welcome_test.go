package launch

import (
	"os"
	"testing"
)

func TestMekugiBuildVersion(t *testing.T) {
	saved := buildVersion
	t.Cleanup(func() { buildVersion = saved })

	buildVersion = "v1.2.3-test"
	if got := mekugiVersion(); got != "v1.2.3-test" {
		t.Fatalf("mekugiVersion() = %q, want %q", got, "v1.2.3-test")
	}
	buildVersion = ""
	if got := mekugiVersion(); got == "" {
		t.Fatal("mekugiVersion() returned empty fallback")
	}
}

func TestMekugiLinkedBuildVersion(t *testing.T) {
	want := os.Getenv("MEKUGI_TEST_BUILD_VERSION")
	if want == "" {
		t.Skip("MEKUGI_TEST_BUILD_VERSION not set")
	}
	if got := mekugiVersion(); got != want {
		t.Fatalf("linked mekugiVersion() = %q, want %q", got, want)
	}
}
