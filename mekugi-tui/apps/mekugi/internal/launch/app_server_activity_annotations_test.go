package launch

import (
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

func TestAppServerSearchResultCounts(t *testing.T) {
	for _, tc := range []struct {
		command, output string
		exit            int
		want            int
	}{
		{"ls", "a.go\nb.go\n", 0, 2},
		{"ls -la src", "total 4\ndrwxr-xr-x 2 user group 0 Oct 1 .\n-rw-r--r-- 1 user group 1 Oct 1 a.go\n", 0, 2},
		{"ls empty", "", 0, 0},
		{"ls -s a", "total 0\n0 one\n", 0, 1},
		{"ls *", "repro.go\n\na:\none\n\nb:\ntwo\n", 0, -1},
		{"ls {a,b}", "a:\none\n\nb:\ntwo\n", 0, -1},
		{`ls $paths`, "a:\none\n\nb:\ntwo\n", 0, -1},
		{`rg 'x|^$' matches.txt`, "x\n\n", 0, 2},
		{"rg --files src", "src/a.go\nsrc/b.go\n", 0, 2},
		{"inspect_file a.go", "1-2 import\n4-6 function main\n", 0, 2},
		{"inspect_file a.go b.go", "--- a.go ---\n1-2 import\n--- b.go ---\n(no outline)\n", 0, 1},
		{"inspect_file --json a.go", `{"ok":true,"truncated":false,"data":{"outline":[{"kind":"function"}]}}`, 0, 1},
		{"inspect_file a.go", "(no outline)\n", 0, 0},
		{"inspect_file missing.go", "inspect_file: missing.go: path does not exist\n", 1, -1},
		{"inspect_file a.go", "1-2 import\noutput truncated\n", 0, -1},
		{"ls -C", "a.go  b.go\n", 0, -1},
		{"ls a b", "a:\none\nb:\ntwo\n", 0, -1},
		{"ls", "ls: no such file\n", 2, -1},
		{"rg --files --null", "a.go\x00b.go\x00", 0, -1},
		{"rg -n needle src", "a.go:1:needle\nb.go:2:needle\n", 0, 2},
		{"grep -n needle a.go", "1:needle\n2:needle\n", 0, 2},
		{"rg needle", "", 1, 0},
		{"rg -c needle", "a.go:2\nb.go:3\n", 0, 5},
		{"rg -n -e needle --glob '*.go' src", "a.go:1:needle\n", 0, 1},
		{"rg -A 1 needle", "a.go:1:needle\na.go-2-context\n", 0, -1},
		{"rg -q needle", "", 0, -1},
		{"rg -r 'a\nb' needle", "a\nb\n", 0, -1},
		{"rg -cl needle 123", "123\n", 0, -1},
		{"rg needle", "one\n[100 bytes omitted]\ntwo\n", 0, -1},
		{"rg needle", "error\n", 2, -1},
		{"rg needle", "Output truncated\na.go:1:needle\n", 0, -1},
		{"rg needle | head -1", "a.go:1:needle\n", 0, -1},
		{"rg needle; echo extra", "a.go:1:needle\nextra\n", 0, -1},
	} {
		t.Run(tc.command+"/"+tc.output, func(t *testing.T) {
			item := binding.ThreadItem{Type: "commandExecution", ID: "s", Command: tc.command, Status: "completed", ExitCode: new(tc.exit), AggregatedOutput: new(tc.output)}
			count := sessionpresentation.SearchResults(item.Observation())
			if tc.want < 0 {
				if count != nil {
					t.Fatalf("invented count %d", *count)
				}
				return
			}
			if count == nil || *count != tc.want {
				t.Fatalf("count %v, want %d", count, tc.want)
			}
			u := newAppServerSessionTestUI(t, t.TempDir())
			appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
			if len(u.attached.source.Entries) != 1 || len(u.testActivity(u.shell.Transcript).blocks(0)) != 1 || u.testActivity(u.shell.Transcript).blocks(0)[0].Results == nil || *u.testActivity(u.shell.Transcript).blocks(0)[0].Results != tc.want {
				t.Fatalf("lost count: %+v", u.attached.source.Entries)
			}
			painted := strings.Join(u.shell.Transcript.Painter.Block(u.testActivity(u.shell.Transcript).blocks(0)[0], 120), "\n")
			if !strings.Contains(painted, activityui.ResultCount(count, u.testActivity(u.shell.Transcript).blocks(0)[0].Verb)) {
				t.Fatalf("not muted: %q", painted)
			}
			restored := newAppServerSessionTestUI(t, t.TempDir())
			restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{item}}})
			if *restored.testActivity(restored.shell.Transcript).blocks(0)[0].Results != tc.want {
				t.Fatal("restored count lost")
			}
		})
	}
	item := binding.ThreadItem{Type: "commandExecution", Command: "rg needle", Status: "completed", ExitCode: new(0)}
	if sessionpresentation.SearchResults(item.Observation()) != nil {
		t.Fatal("missing output is not zero")
	}
	item.AggregatedOutput = new(strings.Repeat("x", 1<<20))
	if sessionpresentation.SearchResults(item.Observation()) != nil {
		t.Fatal("silent cap counted as complete output")
	}
	item.AggregatedOutput, item.ExitCode = nil, new(1)
	if count := sessionpresentation.SearchResults(item.Observation()); count == nil || *count != 0 {
		t.Fatal("host empty-output omission lost no-match result")
	}
}

func TestAppServerWebSearchResults(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	item := map[string]any{"id": "web", "type": "webSearch", "query": "golang"}
	appServerTestNotify(t, u, "item/started", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	if len(u.attached.source.Entries) != 1 || u.testActivity(u.shell.Transcript).blocks(0)[0].Results != nil {
		t.Fatal("started search count")
	}
	item["results"] = []any{map[string]any{"title": "one"}, map[string]any{"title": "two"}}
	appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	if len(u.attached.source.Entries) != 1 || u.testActivity(u.shell.Transcript).blocks(0)[0].Results == nil || *u.testActivity(u.shell.Transcript).blocks(0)[0].Results != 2 {
		t.Fatalf("completed search: %+v", u.attached.source.Entries)
	}
}
