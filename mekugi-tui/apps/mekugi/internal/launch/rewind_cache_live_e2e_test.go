//go:build codex_e2e && e2e

package launch

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"
)

// Verify the first request after native same-thread history replacement, not a follow-up hit.
func TestRewindCacheEligibilityLiveOpenAI(t *testing.T) {
	model := strings.TrimSpace(os.Getenv("MEKUGI_E2E_MODEL"))
	if model == "" {
		t.Fatal("MEKUGI_E2E_MODEL must be set")
	}
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	traceRoot := t.TempDir()
	environment := liveOpenAIEnvironment(t, traceRoot)
	workspace := t.TempDir()
	terminal := startAppResumeTerminal(t, func(ctx context.Context) *exec.Cmd {
		args := []string{"-c", fmt.Sprintf("model=%q", model), "-c", `model_provider="openai"`,
			"-c", "features.plugins=false", "-c", `model_reasoning_effort="low"`}
		cmd := exec.CommandContext(ctx, "mekugi-app-server", args...)
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}, "")

	terminal.awaitReady()
	for _, parts := range []string{"KEEP, _, READY", "KEEP, _, WARM", "DROP, _, ANSWER"} {
		terminal.send(parts + ". Without using tools, join those parts with no spaces and reply with only the result.\r")
		terminal.await(strings.ReplaceAll(parts, ", ", ""))
		terminal.awaitStatus("Completed")
	}
	terminal.send("/rewind\r")
	terminal.awaitMatch("loaded rewind choices", func(frame string) bool {
		return strings.Contains(frame, "Rewind before a user turn") && strings.Contains(frame, "❯ KEEP, _, READY.")
	})
	// The picker lists oldest first. Drop only the latest user turn.
	terminal.send("\x1b[B\x1b[B\r")
	terminal.awaitMatch("rewound history", func(frame string) bool {
		return strings.Contains(frame, "KEEP_WARM") && !strings.Contains(frame, "DROP_ANSWER") &&
			!strings.Contains(frame, "Rewind before a user turn") && strings.Contains(frame, "Ready")
	})
	terminal.send("Without using tools, join these parts with no spaces and reply with only the result: AFTER, _, REWIND.\r")
	terminal.await("AFTER_REWIND")
	terminal.awaitStatus("Completed")
	terminal.quit()
	samples := readLiveInferences(t, traceRoot)
	if len(samples) != 4 {
		t.Fatalf("want three seed calls and one post-rewind call, got %d", len(samples))
	}
	for i, sample := range samples {
		t.Logf("native request %d: input=%d cached=%d", i+1, sample.InputTokens, sample.CachedTokens)
	}
	before, after := samples[1], samples[3]
	if before.CachedTokens == 0 {
		t.Fatal("Main did not establish a cached prefix before rewind")
	}
	if before.Thread == "" || after.Thread != before.Thread {
		t.Error("rewind changed native thread identity")
	}
	if after.contains("DROP") || !after.contains("KEEP_WARM") {
		t.Error("first post-rewind request lost retained history or included excluded history")
	}
	assertLiveCachePrefix(t, before, after)
	if after.CachedTokens == 0 {
		t.Error("first post-rewind request had no cached input despite warmed prefix")
	}
}
