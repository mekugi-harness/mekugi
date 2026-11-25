package launch

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/nativetrace"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/shellinstrumentation"

	activitymodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"

	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
	commandsegments "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/segments"
)

func TestAppServerFrontendCommandClassification(t *testing.T) {
	for _, tc := range []struct{ command, want string }{
		{`env -u BASH_ENV -u MEKUGI_EXEC_TRACK bash -c 'sed -n "28,85p" internal/router/app_server_ui.go; cat internal/router/subagent_activity_pane.go | head -85; rg -n "MCP|spawn_agent" internal/router/subagent_activity.go | head -80'`, "Read `internal/router/app_server_ui.go 28:85`\n\nRead `internal/router/subagent_activity_pane.go`\n\nSearch `MCP|spawn_agent` in `internal/router/subagent_activity.go`"},
		{`/usr/bin/sed -n '2,4p' app.go; /bin/cat other.go | /usr/bin/head -80; /usr/bin/rg needle src 2>/dev/null | /usr/bin/head -20`, "Read `app.go 2:4`\n\nRead `other.go`\n\nSearch `needle` in `src`"},
		{`/usr/bin/env --unset=BASH_ENV LANG=C /bin/sh -c 'go test ./...; cat app.go'`, "Run `go test ./...`\n\nRead `app.go`"},
		{`env -u BASH_ENV /bin/dash -c 'cat "$HOME/app.go"'`, "Read `\"$HOME/app.go\"`"},
		{`env -u BASH_ENV timeout 10 /bin/cat app.go | env LANG=C /usr/bin/sed -n '2,4p'`, "Read `app.go 2:4` (timeout 10)"},
		{`timeout 20 env LANG=C bash -c 'timeout 10 cat app.go; rg needle src'`, "Read `app.go` (timeout 10) (timeout 20)\n\nSearch `needle` in `src` (timeout 20)"},
		{`env -u BASH_ENV bash -c 'set -e; timeout 10 sleep 1; cat app.go'`, "Run (timeout 10)\n" + toolActivityFenced("bash", `env -u BASH_ENV bash -c 'set -e; timeout 10 sleep 1; cat app.go'`)},
		{"sed -n '390,432p' $(go env \\\nGOROOT)/src/encoding/json/v2/arshal_time.go", "Read\n```\n$(go env \\\nGOROOT)/src/encoding/json/v2/arshal_time.go 390:432\n```"},
		{`sed -n '1,2p' "$(project-root)/file.go"`, "Read `\"$(project-root)/file.go\" 1:2`"},
		{"cat /home/yusing/projects/codex/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs | sed -n '45,150p'", "Read `/home/yusing/projects/codex/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs 45:150`"},
		{"inspect_file app.go; mcat app.go 1:20; rg -n needle src | head -30", "Inspect `app.go`\n\nRead `app.go 1:20`\n\nSearch `needle` in `src`"},
		{"timeout 123 cat file.go | head -20", "Read `file.go` (timeout 123)"},
		{"timeout 123 cat file.go | sed -n '1,20p'", "Read `file.go 1:20` (timeout 123)"},
		{"timeout 123 nl -ba file.go | sed -n '1,20p'", "Read `file.go 1:20` (timeout 123)"},
		{"cat file.go | timeout 123 head -20", "Read `file.go` (timeout 123)"},
		{`timeout 123 cat file.go | head -n "$count"`, "Run (timeout 123)\n" + toolActivityFenced("bash", `cat file.go | head -n "$count"`)},
		{"timeout 123 skills-mgr get skill", "Skill `skill` (timeout 123)"},
		{"timeout 123 skills-mgr run skill/script --flag", "Skill `run skill/script --flag` (timeout 123)"},
		{`timeout 123 cat "$HOME/file"`, "Read `\"$HOME/file\"` (timeout 123)"},
		{`timeout --help skills-mgr get skill`, "Run\n" + toolActivityFenced("bash", `timeout --help skills-mgr get skill`)},
		{`timeout "$duration" skills-mgr get skill`, "Run\n" + toolActivityFenced("bash", `timeout "$duration" skills-mgr get skill`)},
		{`timeout 123 skills-mgr get skill > out`, "Run (timeout 123)\n" + toolActivityFenced("bash", `skills-mgr get skill > out`)},
		{`timeout 5 echo "$(timeout 10 probe)"`, "Run (timeout 5)\n" + toolActivityFenced("bash", `echo "$(timeout 10 probe)"`)},
		{`timeout 5 sleep 6`, "Run (timeout 5)\n" + toolActivityFenced("bash", `sleep 6`)},
		{`timeout .5m timeout 1h unknown "arg text" 2>err`, "Run (timeout .5m) (timeout 1h)\n" + toolActivityFenced("bash", `unknown "arg text" 2>err`)},
		{`timeout 123 unknown arg`, "Run (timeout 123)\n" + toolActivityFenced("bash", `unknown arg`)},
		{"skills-mgr get js-ts-best-practices; skills-mgr get user-experience", "Skill `js-ts-best-practices`\n\nSkill `user-experience`"},
		{"skills-mgr run use-modern-go/scripts/run-tool.sh list --go-version 1.27", "Skill `run use-modern-go/scripts/run-tool.sh list --go-version 1.27`"},
		{"mcat app.go 1:20; go test ./...", "Read `app.go 1:20`\n\nRun `go test ./...`"},
	} {
		for _, wrapped := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/wrapped=%t", tc.command, wrapped), func(t *testing.T) {
				command := tc.command
				if wrapped {
					command = shellCommandString("/usr/bin/bash", []string{"-lc", command})
				}
				u := newAppServerSessionTestUI(t, t.TempDir())
				item := binding.ThreadItem{ID: "cmd", Type: "commandExecution", Command: command,
					CommandActions: []binding.CommandAction{{Type: "unknown", Command: command}}}
				for _, method := range []string{"item/started", "item/completed"} {
					appServerTestNotify(t, u, method, map[string]any{"threadId": "main", "turnId": "t", "item": item})
				}
				if len(u.attached.source.Entries) != 1 || u.attached.source.Entries[0].Source.Text != tc.want || u.attached.source.Entries[0].Source.Native.Command != command {
					t.Fatalf("classified live entries: %+v; want %q", u.attached.source.Entries, tc.want)
				}
				appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "child", "turnId": "t", "item": item})
				if len(u.attached.childSource.Entries) != 2 || u.attached.childSource.Entries[1].Source.Text != tc.want || u.attached.childSource.Entries[1].Source.Native.Command != command {
					t.Fatalf("classified child text: %q; want %q", u.attached.childSource.Entries[len(u.attached.childSource.Entries)-1].Source.Text, tc.want)
				}
				restored := newAppServerSessionTestUI(t, t.TempDir())
				restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{item}}})
				if len(restored.attached.source.Entries) != 1 || restored.attached.source.Entries[0].Source.Text != tc.want {
					t.Fatalf("classified restored entries: %+v", restored.attached.source.Entries)
				}
			})
		}
	}
}

func TestAppServerOpaqueCommandWrappers(t *testing.T) {
	for _, command := range []string{
		`env MODE=x -u BASH_ENV cat app.go`,
		`env -C /other cat app.go`, `env PATH=/other cat app.go`, `env -u PATH cat app.go`,
		`env MODE="$(touch marker)" cat app.go`, `env -u "$NAME" cat app.go`,
		`env -u BASH_ENV bash -c "$SCRIPT"`, `env -u BASH_ENV bash -c 'cat "$1"' name app.go`,
		`env -u BASH_ENV bash -c 'cat app.go' > out`, `env -u BASH_ENV bash -c 'cat app.go' &`,
		`env -u BASH_ENV bash -c 'cat app.go' | unknown`, `./cat app.go`,
	} {
		if got := sessionpresentation.CommandText((binding.ThreadItem{Command: command}).Observation(), ""); got != "Run\n"+toolActivityFenced("bash", command) {
			t.Errorf("unsafe wrapper %q classified: %s", command, got)
		}
	}
}

func TestUISnapshotWrappedCommands(t *testing.T) {
	for _, thread := range []string{"main", "child"} {
		u := newAppServerSessionTestUI(t, "/workspace")
		u.clock = func() time.Time { return time.Date(2026, 10, 12, 20, 0, 0, 0, time.Local) }
		view := u.testActivity(u.shell.Transcript)
		if thread == "child" {
			view = u.testActivity(u.shell.Agents)
		}
		view.Clock, view.Painter.Theme = u.clock, diffrender.DarkTheme
		for i, command := range []string{
			`env -u BASH_ENV -u MEKUGI_EXEC_TRACK bash -c 'sed -n "28,85p" internal/router/app_server_ui.go; cat internal/router/subagent_activity_pane.go | head -85; rg -n "MCP|spawn_agent" internal/router/subagent_activity.go | head -80'`,
			`/usr/bin/sed -n '2,4p' app.go; /bin/cat other.go | /usr/bin/head -80`,
			`env -u BASH_ENV bash -c 'set -e; timeout 10 sleep 1; cat app.go'`,
		} {
			item := binding.ThreadItem{ID: fmt.Sprint(i), Type: "commandExecution", Command: command, Cwd: "/workspace", Status: "completed", ExitCode: new(0), DurationMS: new(int64(500))}
			appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": thread, "turnId": "t", "item": item})
		}
		view.testPace(u.clock().Add(time.Hour))
		assertNativeUISnapshot(t, "wrapped-commands-"+thread, view.RenderFeed(100, 30).Lines)
	}
}

func TestAppServerShellDisplay(t *testing.T) {
	for _, tt := range []struct{ input, want string }{
		{`/usr/bin/bash -lc "sed -i 's/a/b/' file; go test ./..."`, `sed -i 's/a/b/' file; go test ./...`},
		{`bash -c 'printf '\''hello'\'''`, `printf 'hello'`},
		{"/bin/bash -lc 'echo one\necho two'", "echo one\necho two"},
		{"bash -lc 'echo ```'", "echo ```"},
		{`bash -lc 'echo hi' > out`, `bash -lc 'echo hi' > out`},
		{`bash -lc 'echo "$1"' name value`, `bash -lc 'echo "$1"' name value`},
		{`bash -lc "$SCRIPT"`, `bash -lc "$SCRIPT"`},
		{`bash -lc 'echo hi'; echo after`, `bash -lc 'echo hi'; echo after`},
		{`ENV=value bash -lc 'echo hi'`, `ENV=value bash -lc 'echo hi'`},
		{`bash -lc 'unfinished`, `bash -lc 'unfinished`},
		{`echo ordinary`, `echo ordinary`},
		{`zsh -lc 'echo hi'`, `zsh -lc 'echo hi'`},
		{`/bin/bash -lc 'python3 -c '\''print("Hello, world!")'\'''`, `python3 -c 'print("Hello, world!")'`},
		{`sh -c 'echo hello'`, `echo hello`},
		{`bash.exe -lc 'echo hi'`, `echo hi`},
		{`powershell.exe -Command 'Write-Host hi'`, `Write-Host hi`},
		{`pwsh -NoLogo -NoProfile -c 'Get-ChildItem | Select-String foo'`, `Get-ChildItem | Select-String foo`},
		{`pwsh -nOpRoFiLe -COMMAND 'Write-Host hi'`, `Write-Host hi`},
		{`pwsh -File script.ps1`, `pwsh -File script.ps1`},
		{`pwsh -Command 'Write-Host $args' extra`, `pwsh -Command 'Write-Host $args' extra`},
		{`pwsh -Unknown -Command 'Write-Host hi'`, `pwsh -Unknown -Command 'Write-Host hi'`},
		{`bash -lc 'echo hi' &`, `bash -lc 'echo hi' &`},
		{`! bash -lc 'echo hi'`, `! bash -lc 'echo hi'`},
		{`bash -lc 'echo hi' | cat`, `bash -lc 'echo hi' | cat`},
	} {
		t.Run(tt.input, func(t *testing.T) {
			item := binding.ThreadItem{Command: tt.input}
			if got := sessionpresentation.DisplayCommand(item.Command); got != tt.want {
				t.Fatalf("display = %q, want %q", got, tt.want)
			}
			if item.Command != tt.input {
				t.Fatal("changed original command")
			}
		})
	}
}

func TestAppServerShellDisplayHighlight(t *testing.T) {
	source := `printf '%s' "$HOME"; go test ./...`
	blocks := parseLiveActivity(sessionpresentation.ActivityEntry{Kind: "tool", Text: sessionpresentation.CommandText((binding.ThreadItem{Command: `/bin/bash -lc 'printf '\''%s'\'' "$HOME"; go test ./...'`}).Observation(), "")})
	for _, theme := range []diffrender.Theme{diffrender.DarkTheme, diffrender.LightTheme} {
		p := activityui.Painter{Theme: theme}
		var rows []string
		for _, block := range blocks {
			rows = append(rows, p.Block(block, 120)...)
		}
		got := strings.Join(rows, "\n")
		highlighted := strings.Join(p.Highlight("bash", source), " ")
		if ansi.Strip(highlighted) != source || highlighted == source || !strings.Contains(ansi.Strip(got), "printf") || !strings.Contains(ansi.Strip(got), "go test ./...") || strings.Contains(ansi.Strip(got), "/bin/bash") {
			t.Fatalf("inner source not highlighted: %q", got)
		}
		for _, block := range blocks {
			highlighted := strings.Join(p.Highlight(block.Lang, block.Code), " ")
			if !strings.Contains(got, highlighted) {
				t.Fatalf("command lost shared highlighting: %q", got)
			}
		}
	}
}

func TestAppServerImageViewLiveAndRestored(t *testing.T) {
	workspace := t.TempDir()
	item := binding.ThreadItem{ID: "image", Type: "imageView", Path: workspace + "/images/a.png"}
	u := newAppServerSessionTestUI(t, workspace)
	thread := "main"
	view := u.testActivity(u.shell.Transcript)

	for _, method := range []string{"item/started", "item/completed"} {
		appServerTestNotify(t, u, method, map[string]any{"threadId": thread, "turnId": "t", "item": item})
	}
	check := func(v *activityAdapter) {
		t.Helper()
		if len(v.Entries) != 1 || v.Entries[0].Source.Text != "View `images/a.png`" {
			t.Fatalf("image activity: %+v", v.Entries)
		}
		if images := v.Entries[0].Source.ActivityFacts().Images; len(images) != 1 || images[0] != item.Path {
			t.Fatalf("image binding: %v", images)
		}
		blocks := parseLiveActivity(v.Entries[0].Source)
		if len(blocks) != 1 || blocks[0].Verb != "View" || len(blocks[0].Reads) != 1 || blocks[0].Reads[0].Path != "images/a.png" {
			t.Fatalf("image blocks: %+v", blocks)
		}
	}
	check(view)
	restored := newAppServerSessionTestUI(t, workspace)
	turns := []binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{item}}}
	restored.restoreHistory(turns)
	check(restored.testActivity(restored.shell.Transcript))
}

func TestAppServerCommandTimeoutInput(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	f := newNativeTraceFixture(t)
	setTestAppServerIntegration(u, &observationFixture{nativeTrace: nativetrace.NewWithNotice(f.root, nil)})
	for _, thread := range []string{"main", "child"} {
		f.start(thread, thread, "outer", "source")
		timeoutMS, wantTimeout := 12500, "12.5s"
		if thread == "child" {
			timeoutMS, wantTimeout = 90000, "90s"
		}
		for _, tc := range []struct {
			id, args, want string
		}{
			{"timed", fmt.Sprintf(`{"cmd":"sleep 1","timeout_ms":%d}`, timeoutMS), wantTimeout},
			{"yielded", `{"cmd":"sleep 1","yield_time_ms":12500}`, ""},
			{"unknown", "", ""},
		} {
			if tc.args != "" {
				f.tool(thread, thread, tc.id, "exec_command", tc.args)
			}
			item := binding.ThreadItem{ID: tc.id, Type: "commandExecution", Command: "sleep 1", DurationMS: new(int64(500))}
			for _, method := range []string{"item/started", "item/completed"} {
				appServerTestNotify(t, u, method, map[string]any{"threadId": thread, "turnId": "t", "item": item})
				view := u.testActivity(u.shell.Transcript)
				if thread != "main" {
					view = u.testActivity(u.shell.Agents)
				}
				entry := view.Entries[len(view.Entries)-1]
				if entry.Source.Native.Command != item.Command || entry.Source.Native.CommandTimeout != tc.want {
					t.Fatalf("%s/%s timeout = %q, want %q", thread, tc.id, entry.Source.Native.CommandTimeout, tc.want)
				}
				blocks := parseLiveActivity(entry.Source)
				if got := strings.Join(blocks[0].Timeouts, " "); got != tc.want {
					t.Fatalf("%s/%s displayed timeout = %q", thread, tc.id, got)
				}
				if method == "item/completed" {
					rendered := strings.Join(view.Painter.Block(view.Disclosure.Blocks(entry.Values)[0], 90), "\n")
					if strings.Contains(rendered, "(timeout ") != (tc.want != "") {
						t.Fatalf("%s/%s rendered timeout: %s", thread, tc.id, rendered)
					}
				}
			}
		}
	}
	restored := newAppServerSessionTestUI(t, t.TempDir())
	restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{{ID: "timed", Type: "commandExecution", Command: "skills-mgr get skill"}}}})
	if len(restored.testActivity(restored.shell.Transcript).blocks(0)[0].Timeouts) != 0 {
		t.Fatal("restored a timeout without original invocation input")
	}
}

func TestUISnapshotTimeoutSkill(t *testing.T) {
	p := activityui.Painter{Theme: diffrender.DarkTheme}
	blocks := parseLiveActivity(sessionpresentation.ActivityEntry{Kind: "tool", Text: toolActivityShell("timeout 123 skills-mgr get skill")})
	blocks[0].Duration = 500 * time.Millisecond
	assertNativeUISnapshot(t, "timeout-skill", p.Block(blocks[0], 90))
}

func TestUISnapshotTimeoutRun(t *testing.T) {
	p := activityui.Painter{Theme: diffrender.DarkTheme}
	blocks := parseLiveActivity(sessionpresentation.ActivityEntry{Kind: "tool", Text: toolActivityShell("timeout 5 sleep 6")})
	blocks[0].Duration, blocks[0].ExitCode = 5*time.Second, 124
	assertNativeUISnapshot(t, "timeout-run", p.Block(blocks[0], 90))
}

func TestUISnapshotShellSetupEvent(t *testing.T) {
	u := newAppServerSessionTestUI(t, "/workspace")
	u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
	command := "probe_root=$(mktemp -d /tmp/setup-apt-baseline.XXXXXX)\n" +
		"trap 'rm -rf \"$probe_root\"' EXIT\nexport SETUP_CONFIG=/workspace/setup.json\n" +
		"smoke_root=\"$probe_root\"\ngit show 99076ca:setup.sh > \"$probe_root/baseline.sh\"\n" +
		"sed -n '1,20p' setup_test.sh\nsource \"$probe_root/baseline.sh\"\ntrap - ERR\n" +
		"source \"$probe_root/policy.sh\"\nif check_apt_policy; then exit 1; fi"
	item := binding.ThreadItem{ID: "setup", Type: "commandExecution", Command: command, Cwd: "/workspace", Status: "completed", ExitCode: new(0)}
	appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	finishPacing(u.testActivity(u.shell.Transcript))
	assertNativeUISnapshot(t, "shell-setup-event", u.testActivity(u.shell.Transcript).RenderFeed(100, 24).Lines)
}

func TestUISnapshotClassifiedCommandPaths(t *testing.T) {
	u := newAppServerSessionTestUI(t, "/workspace")
	u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
	for i, command := range []string{
		"mcat /workspace/internal/router/app.go 1:3",
		"inspect_file /workspace/app.go /external/other.go",
		"rg '/workspace/literal' /workspace/src /external",
	} {
		item := binding.ThreadItem{ID: fmt.Sprint(i), Type: "commandExecution", Command: command, Cwd: "/workspace", Status: "completed", ExitCode: new(0), DurationMS: new(int64(20))}
		appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	}
	finishPacing(u.testActivity(u.shell.Transcript))
	assertNativeUISnapshot(t, "classified-command-paths", u.testActivity(u.shell.Transcript).RenderFeed(100, 30).Lines)
}

func TestLiveActivitySkillRendering(t *testing.T) {
	for _, theme := range []diffrender.Theme{diffrender.DarkTheme, diffrender.LightTheme} {
		p := activityui.Painter{Theme: theme}
		for _, command := range []string{"skills-mgr get golang-best-practices", "skills-mgr run use-modern-go/scripts/run-tool.sh list --go-version 1.27"} {
			blocks := parseLiveActivity(sessionpresentation.ActivityEntry{Kind: "tool", Text: toolActivityShell(command)})
			if len(blocks) != 1 || blocks[0].Verb != "Skill" {
				t.Fatalf("skill blocks: %+v", blocks)
			}
			rows := strings.Join(p.Block(blocks[0], 120), "\n")
			if !strings.Contains(rows, activityui.VerbColor(p.Theme, "Skill")+"\x1b[1mSkill") || activityui.VerbColor(p.Theme, "Skill") == "" {
				t.Fatalf("uncolored skill: %q", rows)
			}
			if strings.Contains(command, " run ") && !strings.Contains(ansi.Strip(rows), "Skill  run use-modern-go/scripts/run-tool.sh list --go-version 1.27") {
				t.Fatalf("skill run spacing: %q", rows)
			}
		}
	}
}

func TestAppServerCommandWorkdirDisplay(t *testing.T) {
	workspace := t.TempDir()
	other := t.TempDir()
	for _, tc := range []struct {
		name, cwd, workdir string
		actions            []binding.CommandAction
		command, text      string
	}{
		{name: "workspace", cwd: workspace, command: "go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "unknown", command: "go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "outside", cwd: other, workdir: other, command: "go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "subdirectory", cwd: filepath.Join(workspace, "sub"), workdir: "sub", command: "cat a.go",
			actions: []binding.CommandAction{{Type: "read", Command: "cat a.go", Path: filepath.Join(workspace, "sub", "a.go")}}, text: "Read `a.go`"},
		{name: "frontend read", cwd: workspace, command: "mcat " + quoteShellWord(workspace+"/a.go") + " 1:3", text: "Read `a.go 1:3`"},
		{name: "inspect other directory", cwd: other, workdir: other, command: "inspect_file " + quoteShellWord(other+"/a.go") + " " + quoteShellWord(workspace+"/b.go"), text: "Inspect `a.go`\n\nInspect `" + workspace + "/b.go`"},
		{name: "search path and literal query", cwd: workspace, command: "rg " + quoteShellWord(workspace+"/literal") + " " + quoteShellWord(workspace+"/src") + " " + quoteShellWord(other), text: "Search `" + workspace + "/literal` in `src` `" + other + "`"},
		{name: "piped read", cwd: workspace, command: "nl -ba " + quoteShellWord(workspace+"/a.go") + " | sed -n '2,4p'", text: "Read `a.go 2:4`"},
		{name: "list", cwd: other, workdir: other, command: "ls src",
			actions: []binding.CommandAction{{Type: "listFiles", Command: "ls src", Path: "src"}}, text: "List `src`"},
		{name: "leading cd subdir", cwd: workspace, workdir: "sub", command: "cd sub && go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "leading cd wrapped bash", cwd: workspace, workdir: "sub", command: "/bin/bash -lc 'cd sub && go test ./...'", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "leading cd outside", cwd: workspace, workdir: other, command: "cd " + quoteShellWord(other) + " && go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "leading cd absolute without host cwd", workdir: other, command: "cd " + quoteShellWord(other) + " && go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "leading cd read action", cwd: workspace, workdir: "sub", command: "cd sub && cat a.go",
			actions: []binding.CommandAction{{Type: "read", Command: "cat a.go", Path: "sub/a.go"}}, text: "Read `a.go`"},
		{name: "leading cd list action", cwd: workspace, workdir: "sub", command: "cd sub && ls src",
			actions: []binding.CommandAction{{Type: "listFiles", Command: "ls src", Path: "src"}}, text: "List `src`"},
		{name: "leading cd search action", cwd: workspace, workdir: "sub", command: "cd sub && rg foo src",
			actions: []binding.CommandAction{{Type: "search", Command: "rg foo src", Query: "foo", Path: "src"}}, text: "Search `foo` in `src`"},
		{name: "leading cd search", cwd: workspace, workdir: "sub", command: "cd sub && rg foo src", text: "Search `foo` in `src`"},
		{name: "leading cd back to workspace", cwd: filepath.Join(workspace, "sub"), command: "cd .. && go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "leading cd semicolon", cwd: workspace, workdir: "sub", command: "cd sub; go test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "leading cd newline", cwd: workspace, workdir: "sub", command: "cd sub\ngo test ./...", text: "Run\n```bash\ngo test ./...\n```"},
		{name: "unsafe or cd kept", cwd: workspace, command: "cd sub && go test ./... || echo fail", text: "Run\n```bash\ncd sub && go test ./... || echo fail\n```"},
		{name: "unsafe later cd back kept", cwd: workspace, command: "cd sub && go test ./... && cd .. && go vet ./...", text: "Run\n```bash\ncd sub && go test ./... && cd .. && go vet ./...\n```"},
		{name: "unsafe cd dash kept", cwd: workspace, command: "cd - && go test ./...", text: "Run\n```bash\ncd - && go test ./...\n```"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if tc.actions == nil {
				tc.actions = []binding.CommandAction{{Type: "unknown", Command: tc.command}}
			}
			item := binding.ThreadItem{ID: "cmd", Type: "commandExecution", Command: tc.command, Cwd: tc.cwd, CommandActions: tc.actions}
			check := func(u *appServerUI) {
				t.Helper()
				if len(u.attached.source.Entries) != 1 || u.attached.source.Entries[0].Source.Text != tc.text || u.attached.source.Entries[0].Source.Native.Workdir != tc.workdir {
					t.Fatalf("entries = %+v; want text %q in %q", u.attached.source.Entries, tc.text, tc.workdir)
				}
				if u.attached.source.Entries[0].Source.Native.Command != tc.command {
					t.Fatal("retained command source changed")
				}
				blocks := parseLiveActivity(u.attached.source.Entries[0].Source)
				if len(blocks) == 0 || blocks[0].Workdir != tc.workdir {
					t.Fatalf("blocks = %+v; want workdir %q", blocks, tc.workdir)
				}
				if tc.name == "leading cd search" && len(blocks) != 1 {
					t.Fatalf("expected single classified block without compound combined-output row, got %+v", blocks)
				}
				// A long workdir moves to its own row, so check the whole block.
				rows := ansi.Strip(strings.Join((&activityui.Painter{Theme: diffrender.DarkTheme}).Block(blocks[0], 200), "\n"))
				if want := "· in " + tc.workdir; (tc.workdir != "") != strings.Contains(rows, want) || tc.workdir == "" && strings.Contains(rows, "· in ") {
					t.Fatalf("rows = %q; want workdir %q", rows, tc.workdir)
				}
			}
			u := newAppServerSessionTestUI(t, workspace)
			for _, method := range []string{"item/started", "item/completed"} {
				appServerTestNotify(t, u, method, map[string]any{"threadId": "main", "turnId": "t", "item": item})
			}
			check(u)
			restored := newAppServerSessionTestUI(t, workspace)
			restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{item}}})
			check(restored)
		})
	}
}

func TestUISnapshotActivityWorkdirRuns(t *testing.T) {
	workspace := t.TempDir()
	dir := filepath.Join(".worktrees", "topic")
	editPath := filepath.Join(dir, "internal", "edit.go")
	workdir := filepath.Join(workspace, dir)
	edit := binding.ThreadItem{ID: "edit", Type: "fileChange", Status: "completed",
		Changes: []binding.FileChange{{Path: filepath.Join(workspace, editPath), Diff: "@@ -1 +1 @@\n-before\n+after\n"}}}
	command := func(id, code, cwd string) binding.ThreadItem {
		return binding.ThreadItem{ID: id, Type: "commandExecution", Command: code, Cwd: cwd, Status: "completed", ExitCode: new(0)}
	}
	failed := command("test", "go test ./...", workdir)
	failed.Status, failed.ExitCode, failed.AggregatedOutput = "failed", new(1), new("test failed\n")
	items := []binding.ThreadItem{
		edit,
		failed,
		{ID: "older-reasoning", Type: "reasoning", Summary: []string{"Hidden reasoning."}},
		command("search", "rg package internal", workdir),
		command("root", "go version", workspace),
		command("one", "go env GOROOT", workdir),
		command("two", "go env GOPATH", workdir),
		{ID: "latest-reasoning", Type: "reasoning", Summary: []string{"Visible reasoning."}},
		command("separate", "go env GOOS", filepath.Join(workspace, "sub")),
	}
	live, restored := newAppServerSessionTestUI(t, workspace), newAppServerSessionTestUI(t, workspace)
	notify := func(method string, params map[string]any) {
		reasoningTestNotify(t, live, method, params)
	}
	notify("turn/started", map[string]any{"threadId": "main", "turn": map[string]any{"id": "t"}})
	for _, item := range items {
		notify("item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	}
	notify("turn/completed", map[string]any{"threadId": "main", "turn": map[string]any{"id": "t", "status": "completed"}})
	restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: items}})
	for _, ui := range []*appServerUI{live, restored} {
		view := ui.testActivity(ui.shell.Transcript)
		view.Conversation = true
		feed := view.testRenderConversation(100)
		if !slices.ContainsFunc(feed.Snippets, func(s activityui.Snippet) bool {
			return s.Block == activityui.EditNavigationSnippet && s.Path == editPath
		}) {
			t.Fatal("the compact edit label lost its original navigation path")
		}
		assertNativeUISnapshot(t, "activity-workdir-runs", feed.Lines)
		var scrolled []string
		view.Following = false
		for _, label := range []string{"Edited", "Search", "go env GOROOT"} {
			view.Offset = slices.IndexFunc(feed.Lines, func(line string) bool {
				return strings.Contains(ansi.Strip(line), label)
			})
			if view.Offset < 0 {
				t.Fatalf("missing scroll position for %q", label)
			}
			scrolled = append(scrolled, view.Viewport(feed, 4)...)
			scrolled = append(scrolled, "")
		}
		assertNativeUISnapshot(t, "activity-workdir-pinned", scrolled)
		view.Conversation = false
		activity := view.RenderFeed(100, 40)
		rows := ansi.Strip(strings.Join(activity.Lines, "\n"))
		if strings.Count(rows, "In "+dir) != 2 || strings.Contains(rows, "· in "+dir) || !strings.Contains(rows, "internal"+string(filepath.Separator)+"edit.go") {
			t.Fatalf("the activity feed did not preserve the directory groups:\n%s", rows)
		}
		view.Offset = slices.IndexFunc(activity.Lines, func(line string) bool {
			return strings.Contains(ansi.Strip(line), "Edited")
		})
		if visible := view.Viewport(activity, 4); !strings.Contains(ansi.Strip(visible[0]), "In "+dir) {
			t.Fatalf("activity pinned the wrong heading:\n%s", strings.Join(visible, "\n"))
		}
	}
}

func TestAppServerLeadingCdWorkdirGroupingAndParity(t *testing.T) {
	workspace := t.TempDir()
	dir := filepath.Join(".worktrees", "topic")
	editPath := filepath.Join(dir, "internal", "edit.go")
	workdir := filepath.Join(workspace, dir)
	edit := binding.ThreadItem{ID: "edit", Type: "fileChange", Status: "completed",
		Changes: []binding.FileChange{{Path: filepath.Join(workspace, editPath), Diff: "@@ -1 +1 @@\n-before\n+after\n"}}}
	command := func(id, code, cwd string) binding.ThreadItem {
		return binding.ThreadItem{ID: id, Type: "commandExecution", Command: code, Cwd: cwd, Status: "completed", ExitCode: new(0)}
	}
	items := []binding.ThreadItem{
		edit,
		command("cd-test", "cd "+dir+" && go test ./...", workspace),
		command("native-search", "rg package internal", workdir),
		command("cd-env", "/bin/bash -lc 'cd "+dir+" && go env GOPATH'", workspace),
		command("root", "go version", workspace),
		command("single-cd", "cd sub && go env GOOS", workspace),
	}
	live, restored := newAppServerSessionTestUI(t, workspace), newAppServerSessionTestUI(t, workspace)
	notify := func(method string, params map[string]any) {
		reasoningTestNotify(t, live, method, params)
	}
	notify("turn/started", map[string]any{"threadId": "main", "turn": map[string]any{"id": "t"}})
	for _, item := range items {
		notify("item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	}
	notify("turn/completed", map[string]any{"threadId": "main", "turn": map[string]any{"id": "t", "status": "completed"}})
	restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: items}})
	for _, ui := range []*appServerUI{live, restored} {
		view := ui.testActivity(ui.shell.Transcript)
		view.Conversation = true
		conv := view.testRenderConversation(100)
		convRows := ansi.Strip(strings.Join(conv.Lines, "\n"))
		if strings.Contains(convRows, "cd "+dir) || strings.Contains(convRows, "cd sub") {
			t.Fatalf("leading cd was not omitted in conversation view:\n%s", convRows)
		}
		if strings.Count(convRows, "In "+dir) != 1 || strings.Contains(convRows, "· in "+dir) {
			t.Fatalf("leading cd commands did not group under one In %s heading:\n%s", dir, convRows)
		}
		if !strings.Contains(convRows, "internal"+string(filepath.Separator)+"edit.go") {
			t.Fatalf("adopted edit path was not shortened relative to %s:\n%s", dir, convRows)
		}
		if !strings.Contains(convRows, "· in sub") {
			t.Fatalf("standalone leading cd command missing inline workdir label:\n%s", convRows)
		}
		view.Following = false
		view.Offset = slices.IndexFunc(conv.Lines, func(line string) bool {
			return strings.Contains(ansi.Strip(line), "Search")
		})
		if view.Offset < 0 {
			t.Fatalf("missing Search row in:\n%s", convRows)
		}
		if visible := view.Viewport(conv, 4); !strings.Contains(ansi.Strip(visible[0]), "In "+dir) {
			t.Fatalf("pinned heading missing for leading-cd group:\n%s", strings.Join(visible, "\n"))
		}
		view.Conversation = false
		activity := view.RenderFeed(100, 40)
		actRows := ansi.Strip(strings.Join(activity.Lines, "\n"))
		if strings.Count(actRows, "In "+dir) != 1 || strings.Contains(actRows, "· in "+dir) || !strings.Contains(actRows, "· in sub") {
			t.Fatalf("activity feed did not preserve leading-cd workdir behavior:\n%s", actRows)
		}
	}
}

func TestLeadingCdSearchResultsAndVCSCommitDirectory(t *testing.T) {
	workspace := t.TempDir()
	searchOut := "a.go:1:foo\nb.go:2:foo\n"
	searchItem := (binding.ThreadItem{Type: "commandExecution", Command: "cd sub && rg foo src", Cwd: workspace, Status: "completed", ExitCode: new(0), AggregatedOutput: &searchOut}).Observation()
	if count := sessionpresentation.SearchResults(searchItem); count == nil || *count != 2 {
		t.Fatalf("SearchResults = %v; want 2", count)
	}
	commitOut := "[main abc1234] msg\n 1 file changed, 2 insertions(+), 1 deletion(-)\n"
	commitItem := (binding.ThreadItem{Type: "commandExecution", Command: "cd sub && git commit -m msg", Cwd: workspace, Status: "completed", ExitCode: new(0), AggregatedOutput: &commitOut}).Observation()
	rows, key := sessionpresentation.CommandOutputRows(commitItem)
	wantDir := filepath.Join(workspace, "sub")
	if len(rows) != 1 || key.Directory != wantDir || key.Hash != "abc1234" {
		t.Fatalf("CommandOutputRows = (%+v, %+v); want 1 row in %q hash abc1234", rows, key, wantDir)
	}
}

func TestLeadingCdFailedCdKeepsOriginalCommandAndWorkdir(t *testing.T) {
	workspace := t.TempDir()
	for _, tc := range []struct {
		name, command, status, output, wantWorkdir, wantText string
		exitCode                                             int
	}{
		{"and chain cd failure", "cd missing && go test ./...", "failed", "bash: line 1: cd: missing: No such file or directory\n", "", "cd missing && go test ./...", 1},
		{"semicolon cd failure with zero exit", "cd missing; pwd", "completed", "bash: line 1: cd: missing: No such file or directory\n" + workspace + "\n", "", "cd missing; pwd", 0},
		{"nested child script cd error preserves outer workdir", "cd sub && ./build.sh", "failed", "./build.sh: line 2: cd: generated: No such file or directory\n", "sub", "./build.sh", 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			out := tc.output
			item := binding.ThreadItem{ID: "cmd", Type: "commandExecution", Command: tc.command, Cwd: workspace, Status: tc.status, ExitCode: new(tc.exitCode), AggregatedOutput: &out}
			live, restored := newAppServerSessionTestUI(t, workspace), newAppServerSessionTestUI(t, workspace)
			for _, method := range []string{"item/started", "item/completed"} {
				appServerTestNotify(t, live, method, map[string]any{"threadId": "main", "turnId": "t", "item": item})
			}
			restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{item}}})
			for _, ui := range []*appServerUI{live, restored} {
				entry := ui.attached.source.Entries[0].Source
				if entry.Native.Workdir != tc.wantWorkdir || !strings.Contains(entry.Text, tc.wantText) {
					t.Fatalf("got workdir=%q text=%q; want workdir=%q containing %q", entry.Native.Workdir, entry.Text, tc.wantWorkdir, tc.wantText)
				}
			}
		})
	}
}

func TestLeadingCdTrackedSegmentsOmitCd(t *testing.T) {
	workspace := t.TempDir()
	for _, script := range []string{"cd sub && go test ./...", "cd sub; go test ./...", "cd sub\ngo test ./...\n"} {
		parts, ok := shellinstrumentation.Split(script)
		if !ok || len(parts) != 2 {
			t.Fatalf("Split(%q) = (%+v, %v); want 2 segments", script, parts, ok)
		}
		item := (binding.ThreadItem{ID: "tracked", Type: "commandExecution", Command: script, Cwd: workspace, Status: "completed", ExitCode: new(0)}).Observation()
		cmd := activitypresentation.Command{
			Source: 1, Script: item.Command, Display: sessionpresentation.CommandDisplaySource(item), Directory: sessionpresentation.CommandDirectory(item, workspace),
			Segments: []activitypresentation.CommandSegment{
				{Source: parts[0].Source, Display: parts[0].Source, Text: activitypresentation.RunSource(parts[0].Source), ExitCode: 0},
				{Source: parts[1].Source, Display: parts[1].Source, Text: activitypresentation.RunSource(parts[1].Source), ExitCode: 0},
			},
		}
		values := cmd.SegmentValues(false)
		if len(values) != 1 || strings.Contains(values[0].Code, "cd sub") || strings.Contains(values[0].Label, "cd sub") {
			t.Fatalf("SegmentValues(%q) = %+v; want only go test without cd sub", script, values)
		}
		cmd.Segments[0].ExitCode = 1
		cmd.Segments[1].Skipped = true
		failedValues := cmd.SegmentValues(false)
		if len(failedValues) < 1 || !strings.Contains(failedValues[0].Code+failedValues[0].Label, "cd sub") {
			t.Fatalf("failed cd segment was unexpectedly hidden: %+v", failedValues)
		}
	}
	// Shell-setup script kept as one segment by Split (with trailing newline trimmed).
	grouped := "cd sub && export FOO=1 && go test ./...\n"
	parts, ok := shellinstrumentation.Split(grouped)
	if !ok || len(parts) != 1 {
		t.Fatalf("Split(%q) = (%+v, %v); want 1 grouped segment", grouped, parts, ok)
	}
	item := (binding.ThreadItem{ID: "grouped", Type: "commandExecution", Command: grouped, Cwd: workspace, Status: "completed", ExitCode: new(0)}).Observation()
	dir, display := sessionpresentation.CommandDirectory(item, workspace), sessionpresentation.CommandDisplaySource(item)
	inv := activitypresentation.NewInvocation(activitypresentation.CommandSource{Script: item.Command, Display: display, Directory: dir}, time.Time{}, nil)
	report := sessionpresentation.CommandReport(inv, commandsegments.Report{
		Terminal: true, Done: true, Code: 0,
		Segments: []commandsegments.Segment{{Source: parts[0].Source, Began: true, Ended: true, Code: 0}},
	}, dir)
	if len(report.Segments) != 1 || report.Segments[0].Analysis != "export FOO=1 && go test ./..." || strings.Contains(report.Segments[0].Text, "cd sub") {
		t.Fatalf("grouped tracked segment re-exposed leading cd: %+v", report.Segments)
	}
	if restored := sessionpresentation.SingleSegmentDisplay(parts[0].Source, display, item.Cwd); restored != "export FOO=1 && go test ./..." {
		t.Fatalf("SingleSegmentDisplay = %q; want omitted leading cd", restored)
	}
}

func TestUISnapshotMCatSourceWithoutFilesystemDisambiguation(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the unrelated literal-colon filename fixture is unavailable on Windows")
	}
	packageDirectory, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	workspace := t.TempDir()
	t.Chdir(t.TempDir())
	if err := os.WriteFile("source:12", []byte("unrelated content\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ name, command, text string }{
		{"ambiguous", "mcat source:12", "Run\n```bash\nmcat source:12\n```"},
		{"separate-range", "mcat source.go 1:3", "Read `source.go 1:3`"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			item := binding.ThreadItem{ID: "cmd", Type: "commandExecution", Command: tc.command, Cwd: workspace, Status: "completed", ExitCode: new(0), DurationMS: new(int64(20))}
			live, restored := newAppServerSessionTestUI(t, workspace), newAppServerSessionTestUI(t, workspace)
			for _, method := range []string{"item/started", "item/completed"} {
				appServerTestNotify(t, live, method, map[string]any{"threadId": "main", "turnId": "t", "item": item})
			}
			restored.restoreHistory([]binding.HistoryTurn{{ID: "t", Status: "completed", Items: []binding.ThreadItem{item}}})
			t.Chdir(packageDirectory)
			for name, ui := range map[string]*appServerUI{"live": live, "restored": restored} {
				if len(ui.attached.source.Entries) != 1 {
					t.Fatalf("%s entries = %+v", name, ui.attached.source.Entries)
				}
				entry := ui.attached.source.Entries[0].Source
				if entry.Text != tc.text || entry.Native.Command != tc.command {
					t.Fatalf("%s source = %q, command = %q; want %q, %q", name, entry.Text, entry.Native.Command, tc.text, tc.command)
				}
				blocks := parseLiveActivity(entry)
				assertNativeUISnapshot(t, "mcat-source-"+tc.name, (&activityui.Painter{Theme: diffrender.DarkTheme}).Block(blocks[0], 80))
			}
		})
	}
}

func TestActivityReadsKeepInvocationWorkdirsApart(t *testing.T) {
	other := t.TempDir()
	u := newAppServerSessionTestUI(t, t.TempDir())
	for _, item := range []binding.ThreadItem{
		{ID: "a", Type: "commandExecution", Command: "rg x; cat a.go", Cwd: other, CommandActions: []binding.CommandAction{{Type: "search", Command: "rg x", Query: "x"}, {Type: "read", Command: "cat a.go", Path: other + "/a.go"}}},
		{ID: "b", Type: "commandExecution", Command: "cat b.go", Cwd: u.attached.cwd, CommandActions: []binding.CommandAction{{Type: "read", Command: "cat b.go", Path: u.attached.cwd + "/b.go"}}},
	} {
		appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
	}
	var blocks []activityui.Block
	for _, entry := range u.attached.source.Entries {
		blocks = append(blocks, parseLiveActivity(entry.Source)...)
	}
	if len(blocks) != 3 || !blocks[0].ShowWorkdir || blocks[1].ShowWorkdir || blocks[1].Workdir != other {
		t.Fatalf("blocks = %+v", blocks)
	}
	if merged := activityui.MergeTargets(blocks); len(merged) != 3 {
		t.Fatalf("reads across directories merged: %+v", merged)
	}
}

func TestActivityReadsKeepWorkdirsApart(t *testing.T) {
	read := func(workdir string) activityui.Block {
		return activityui.Block{Kind: "reads", Verb: "Read", Reads: []activitymodel.Read{{Path: "a.go"}}, Workdir: workdir}
	}
	if merged := activityui.MergeTargets([]activityui.Block{read(""), read("/other")}); len(merged) != 2 {
		t.Fatalf("reads in different directories merged: %+v", merged)
	}
	if merged := activityui.MergeTargets([]activityui.Block{read("/other"), read("/other")}); len(merged) != 1 {
		t.Fatalf("reads in one directory did not merge: %+v", merged)
	}
}

func TestAppServerWholeScriptTimeoutSuffix(t *testing.T) {
	for command, code := range map[string]string{
		// An inline program shows its own source; the invocation keeps its bound.
		"timeout 10 bash <<'EOF'\nsleep 1\nEOF":    "sleep 1",
		"set -e\ntimeout 10 sleep 1":               "set -e\ntimeout 10 sleep 1",
		"set -e\necho start && timeout 10 sleep 1": "set -e\necho start && timeout 10 sleep 1",
	} {
		t.Run(command, func(t *testing.T) {
			item := binding.ThreadItem{ID: "cmd", Type: "commandExecution", Command: command, DurationMS: new(int64(500))}
			u := newAppServerSessionTestUI(t, t.TempDir())
			appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "t", "item": item})
			entry := u.attached.source.Entries[0]
			blocks := parseLiveActivity(entry.Source)
			var p activityui.Painter
			var rows []string
			for _, block := range blocks {
				rows = append(rows, p.Block(block, 90)...)
			}
			if got := strings.Join(rows, "\n"); !strings.Contains(got, "(timeout 10s)") {
				t.Fatalf("missing timeout suffix: %s", got)
			}
			if entry.Source.Native.Command != command || blocks[0].Code != code {
				t.Fatalf("whole script source changed: %+v", blocks[0])
			}
		})
	}
}

func TestUISnapshotShellSetupTimeoutEvent(t *testing.T) {
	for _, thread := range []string{"main", "child"} {
		u := newAppServerSessionTestUI(t, "/workspace")
		now := time.Date(2026, 10, 12, 20, 0, 0, 0, time.Local)
		u.clock = func() time.Time { return now }
		view := u.testActivity(u.shell.Transcript)
		if thread == "child" {
			view = u.testActivity(u.shell.Agents)
		}
		view.Clock, view.Painter.Theme = u.clock, diffrender.DarkTheme
		item := binding.ThreadItem{ID: "setup", Type: "commandExecution", Command: "set -e\ntimeout 10 sleep 1", Cwd: "/workspace", Status: "inProgress"}
		appServerTestNotify(t, u, "item/started", map[string]any{"threadId": thread, "turnId": "t", "item": item})
		now = now.Add(500 * time.Millisecond)
		assertNativeUISnapshot(t, "timeout-setup-"+thread+"-running", view.RenderFeed(70, 24).Lines)
		item.Status, item.ExitCode, item.DurationMS = "completed", new(0), new(int64(500))
		appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": thread, "turnId": "t", "item": item})
		assertNativeUISnapshot(t, "timeout-setup-"+thread+"-completed", view.RenderFeed(70, 24).Lines)
	}
}
