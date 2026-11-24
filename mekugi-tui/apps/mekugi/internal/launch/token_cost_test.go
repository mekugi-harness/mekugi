package launch

import (
	"testing"

	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

func TestFormatUsageTokens(t *testing.T) {
	for value, want := range map[uint64]string{0: "0", 12: "12", 123: "123", 1234: "1.2K", 149000: "149K", 1234567: "1.2M", 1200000000: "1.2B", ^uint64(0): "18446744073.7B"} {
		if got := activityui.FormatUsageTokens(value); got != want {
			t.Errorf("%d: %q != %q", value, got, want)
		}
	}
}
