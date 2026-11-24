package main

import (
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/shellhook"
)

// trackingHook reads the host event, then binds resources from the trusted command
// arguments. Event metadata cannot replace the tracker.
func trackingHook(args []string, input io.Reader, output, stderr io.Writer) int {
	fail := func(err error) int {
		fmt.Fprintln(stderr, "mekugi: shell instrumentation failed:", err)
		return 2
	}
	if len(args) != 1 {
		return fail(errors.New("expected tracker"))
	}
	const limit = 4 << 20
	data, err := io.ReadAll(io.LimitReader(input, limit+1))
	if err != nil {
		return fail(err)
	}
	if len(data) > limit {
		return fail(errors.New("hook input exceeds 4 MiB"))
	}
	var request shellhook.Input
	if err := json.Unmarshal(data, &request); err != nil {
		return fail(err)
	}
	result, err := shellhook.Rewrite(request, args[0])
	if err != nil {
		return fail(err)
	}
	data, err = json.Marshal(result)
	if err != nil {
		return fail(err)
	}
	if _, err := output.Write(append(data, '\n')); err != nil {
		return fail(err)
	}
	return 0
}
