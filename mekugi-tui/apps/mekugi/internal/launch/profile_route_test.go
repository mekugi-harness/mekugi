package launch

import (
	"strings"
	"testing"
)

func TestHeadlessProfileSelector(t *testing.T) {
	for _, args := range [][]string{{"-pwork", "headless", "--yolo"}, {"headless", "-p", "work", "--yolo"}} {
		got, ok := headlessCommand(args)
		if !ok || strings.Contains(strings.Join(got, " "), "headless") {
			t.Fatalf("headless global selector: %q", got)
		}
	}

}
