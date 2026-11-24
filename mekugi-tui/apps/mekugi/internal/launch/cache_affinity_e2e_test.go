//go:build e2e

package launch

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"
)

const cacheAffinityE2EPrompt = `Call the exec tool exactly 12 times, sequentially and never in parallel. Call 1 must run printf 'probe 1\n', call 2 must run printf 'probe 2\n', and continue through call 12 running printf 'probe 12\n'. Do not combine calls. Do not edit any file. After the twelfth tool result, answer only: done`

func TestCodexCacheAffinityE2E(t *testing.T) {
	model := strings.TrimSpace(os.Getenv("MEKUGI_E2E_MODEL"))
	if model == "" {
		t.Fatal("MEKUGI_E2E_MODEL must be set to run live Codex E2E tests")
	}
	codexPath := requireExecutable(t, "codex")
	gitPath := requireExecutable(t, "git")
	workspace := t.TempDir()
	command := exec.Command(gitPath, "init", "--quiet")
	command.Dir = workspace
	command.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("initialize probe repository: %v\n%s", err, output)
	}

	traceRoot := t.TempDir()
	environment := liveOpenAIEnvironment(t, traceRoot)
	ctx, cancel := context.WithTimeout(t.Context(), 15*time.Minute)
	defer cancel()
	args := []string{
		"-c", `model_provider="openai"`,
		"--model", model,
		"--sandbox", "read-only",
		"--ask-for-approval", "never",
		"exec",
		"--ephemeral",
		"--ignore-user-config",
		"--json",
		"--color", "never",
		"-C", workspace,
		cacheAffinityE2EPrompt,
	}
	var stdout, stderr bytes.Buffer
	codex := exec.CommandContext(ctx, codexPath, args...)
	codex.Env = environment
	codex.Stdout = &stdout
	codex.Stderr = &stderr
	if err := codex.Run(); err != nil {
		t.Fatalf(
			"run Codex cache-affinity E2E: %v\nstdout tail:\n%s\nstderr tail:\n%s",
			err,
			tailBytes(stdout.Bytes(), 64<<10),
			tailBytes(stderr.Bytes(), 64<<10),
		)
	}

	usages := readLiveInferences(t, traceRoot)
	if len(usages) < 8 {
		t.Fatalf("captured %d native usage samples, want at least 8", len(usages))
	}
	first := usages[0]
	key := first.Fields["prompt_cache_key"]
	if first.Thread == "" || first.Turn == "" || len(key) == 0 || string(key) == "null" || string(key) == `""` {
		t.Fatal("native trace omitted inference identity or cache key")
	}
	for i, sample := range usages {
		if sample.Thread != first.Thread || sample.Turn != first.Turn ||
			sample.Provider != first.Provider || !bytes.Equal(key, sample.Fields["prompt_cache_key"]) {
			t.Fatalf("native inference %d changed thread, turn, provider, or cache identity", i+1)
		}
	}

	var totalInput, totalCached, warmInput, warmCached uint64
	var warmFullMisses int
	for index, usage := range usages {
		if usage.InputTokens == 0 {
			t.Fatalf("native request %d omitted input token usage", index+1)
		}
		cached := usage.CachedTokens
		totalInput += usage.InputTokens
		totalCached += cached
		if index > 0 {
			warmInput += usage.InputTokens
			warmCached += cached
			if cached == 0 {
				warmFullMisses++
			}
		}

		t.Logf(
			"request %02d: input=%d cached=%d uncached=%d",
			index+1,
			usage.InputTokens,
			cached,
			usage.InputTokens-cached,
		)
	}
	totalRatio := float64(totalCached) / float64(totalInput)
	warmRatio := float64(warmCached) / float64(warmInput)
	t.Logf("cache ratio: %.2f%% (%d/%d input tokens)", 100*totalRatio, totalCached, totalInput)
	t.Logf("warm cache ratio: %.2f%% (%d/%d input tokens)", 100*warmRatio, warmCached, warmInput)
	t.Logf("warm full misses: %d/%d requests", warmFullMisses, len(usages)-1)
}
