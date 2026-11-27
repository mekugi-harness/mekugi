//go:build codex_e2e && e2e

package launch

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"
)

// A side fork's first inference must reuse its already-cached Main prefix.
// Subsequent Main and fork turns preserve their own prefixes in either order.
func TestSideCacheEligibilityLiveOpenAI(t *testing.T) {
	model := strings.TrimSpace(os.Getenv("MEKUGI_E2E_MODEL"))
	if model == "" {
		t.Fatal("MEKUGI_E2E_MODEL must be set")
	}
	for _, command := range []string{"/btw", "/oneoff"} {
		for _, order := range []string{"main-first", "fork-first"} {
			t.Run(strings.TrimPrefix(command, "/")+"/"+order, func(t *testing.T) {
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
				terminal.send("Without using tools, join these parts with no spaces and reply with only the result: MAIN, _, READY.\r")
				terminal.await("MAIN_READY")
				terminal.awaitStatus("Completed")
				terminal.send("Without using tools, join these parts with no spaces and reply with only the result: MAIN, _, WARM.\r")
				terminal.await("MAIN_WARM")
				terminal.awaitStatus("Completed")
				prompt := "Without using tools, join these parts with no spaces and reply with only the result: SIDE, _, READY."
				terminal.send(command + " " + prompt + "\r")
				terminal.await("SIDE_READY")
				if command == "/btw" {
					terminal.await("/btw · completed")
				} else {
					terminal.awaitStatus("completed")
				}
				sequence := []string{"main", "fork"}
				if order == "fork-first" {
					sequence[0], sequence[1] = sequence[1], sequence[0]
				}
				for _, branch := range sequence {
					marker, parts := "MAIN_NEXT", "MAIN, _, NEXT"
					if branch == "fork" {
						marker, parts = "SIDE_NEXT", "SIDE, _, NEXT"
					}
					if command != "/btw" {
						label, selectView := "Main", "\x021"
						if branch == "fork" {
							label, selectView = "Side", "\x1b[<0;12;30M\x1b[<0;12;30m"
							if command == "/oneoff" {
								label = "One-off"
							}
						}
						terminal.send(selectView)
						terminal.awaitMatch(label+" selected", func(frame string) bool {
							return statLineNames(frame, label)
						})
					}
					prompt := "Without using tools, join these parts with no spaces and reply with only the result: " + parts + "."
					if branch == "fork" && command == "/btw" {
						prompt = "/btw " + prompt
					}
					terminal.send(prompt + "\r")
					terminal.await(marker)
					if branch == "main" {
						terminal.awaitStatus("Completed")
					} else if command == "/btw" {
						terminal.await("/btw · completed")
					} else {
						terminal.awaitStatus("completed")
					}
				}
				terminal.quit()
				samples := readLiveInferences(t, traceRoot)
				if len(samples) != 5 {
					t.Fatalf("want two Main turns, a fork turn, and two continuations; got %d calls", len(samples))
				}
				main := samples[1]
				fork := samples[2]
				if main.Thread == "" || samples[0].Thread != main.Thread || fork.Thread == "" || fork.Thread == main.Thread {
					t.Fatal("fork did not get its own native thread identity")
				}
				if main.CachedTokens == 0 {
					t.Fatal("Main did not establish a cached prefix before the fork")
				}
				assertLiveCachePrefix(t, main, fork)
				prior := map[string]liveInference{"main": main, "fork": fork}
				for i, branch := range sequence {
					sample := samples[3+i]
					if prior[branch].Thread != sample.Thread {
						t.Fatalf("%s continuation changed native thread identity", branch)
					}
					assertLiveCachePrefix(t, prior[branch], sample)
					for _, other := range []string{"SIDE_READY", "SIDE_NEXT", "MAIN_NEXT"} {
						if (branch == "main" && strings.HasPrefix(other, "SIDE_") || branch == "fork" && other == "MAIN_NEXT") && sample.contains(other) {
							t.Errorf("%s continuation contains the other branch's %s", branch, other)
						}
					}
				}
				for i, sample := range samples {
					t.Logf("native request %d: input=%d cached=%d", i+1, sample.InputTokens, sample.CachedTokens)
					if i >= 2 && sample.CachedTokens == 0 {
						t.Errorf("request %d had no cached input in %s order", i+1, order)
					}
				}
			})
		}
	}
}

func assertLiveCachePrefix(t *testing.T, before, after liveInference) {
	t.Helper()
	if before.Provider != after.Provider {
		t.Fatal("native inference changed providers")
	}
	for _, field := range []string{"instructions", "tools", "model", "reasoning", "prompt_cache_key"} {
		if !sameLiveRequestValue(before.Fields[field], after.Fields[field]) {
			t.Errorf("native inference changed shared %s", field)
		}
	}
	if len(before.Input) == 0 || len(after.Input) <= len(before.Input) {
		t.Fatal("native inference lost retained context or its new input")
	}
	for i, item := range before.Input {
		if !sameLiveRequestValue(item, after.Input[i]) {
			t.Errorf("native inference changed retained input item %d", i)
		}
	}
}

func sameLiveRequestValue(left, right jsontext.Value) bool {
	if bytes.Equal(left, right) {
		return true
	}
	// Normalize object order and string escapes, but preserve number spelling
	// and leave the captured values intact.
	left, right = left.Clone(), right.Clone()
	order := jsontext.ReorderRawObjects(true)
	return left.Format(order) == nil && right.Format(order) == nil && bytes.Equal(left, right)
}
