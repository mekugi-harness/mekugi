package shellsyntax

import "testing"

func TestStripLeadingCd(t *testing.T) {
	tests := []struct {
		name      string
		script    string
		dir       string
		wantRest  string
		wantDir   string
		wantMatch bool
	}{
		{name: "relative subdir", script: "cd sub && go test ./...", dir: "/workspace", wantRest: "go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "absolute target without base dir", script: "cd /other/repo && git status", dir: "", wantRest: "git status", wantDir: "/other/repo", wantMatch: true},
		{name: "quoted path with spaces", script: "cd 'my dir/sub' && rg foo src", dir: "/workspace", wantRest: "rg foo src", wantDir: "/workspace/my dir/sub", wantMatch: true},
		{name: "double dash separator", script: "cd -- -weird && ls", dir: "/workspace", wantRest: "ls", wantDir: "/workspace/-weird", wantMatch: true},
		{name: "chained commands after cd", script: "cd sub && go vet ./... && go test ./...", dir: "/workspace", wantRest: "go vet ./... && go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "multiline after and", script: "cd sub &&\n  go test ./...", dir: "/workspace", wantRest: "go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "parent resolution back to workspace", script: "cd .. && go test ./...", dir: "/workspace/sub", wantRest: "go test ./...", wantDir: "/workspace", wantMatch: true},
		{name: "heredoc after cd", script: "cd sub && cat <<'EOF'\nhello\nEOF", dir: "/workspace", wantRest: "cat <<'EOF'\nhello\nEOF", wantDir: "/workspace/sub", wantMatch: true},
		{name: "subshell body after cd", script: "cd sub && (go vet ./...; go test ./...)", dir: "/workspace", wantRest: "(go vet ./...; go test ./...)", wantDir: "/workspace/sub", wantMatch: true},
		{name: "semicolon separator", script: "cd sub; go test ./...", dir: "/workspace", wantRest: "go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "semicolon multiple statements", script: "cd sub; go vet ./...; go test ./...", dir: "/workspace", wantRest: "go vet ./...; go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "semicolon then and chain", script: "cd sub; go vet ./... && go test ./...", dir: "/workspace", wantRest: "go vet ./... && go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "newline separator", script: "cd sub\ngo test ./...", dir: "/workspace", wantRest: "go test ./...", wantDir: "/workspace/sub", wantMatch: true},
		{name: "bare cd", script: "cd && ls", dir: "/workspace"},
		{name: "bare cd semicolon", script: "cd; ls", dir: "/workspace"},
		{name: "standalone cd", script: "cd sub", dir: "/workspace"},
		{name: "cd dash", script: "cd - && ls", dir: "/workspace"},
		{name: "cd dash semicolon", script: "cd -; ls", dir: "/workspace"},
		{name: "cd option flag", script: "cd -P sub && ls", dir: "/workspace"},
		{name: "cd tilde", script: "cd ~/repo && ls", dir: "/workspace"},
		{name: "dynamic var", script: "cd \"$DIR\" && ls", dir: "/workspace"},
		{name: "dynamic var semicolon", script: "cd \"$DIR\"; ls", dir: "/workspace"},
		{name: "command substitution", script: "cd $(pwd)/sub && ls", dir: "/workspace"},
		{name: "or fallback", script: "cd sub || exit 1", dir: "/workspace"},
		{name: "and then or fallback", script: "cd sub && go test ./... || echo fail", dir: "/workspace"},
		{name: "and then semicolon", script: "cd sub && go test ./...; echo done", dir: "/workspace"},
		{name: "subshell wrapping cd", script: "(cd sub && go test ./...)", dir: "/workspace"},
		{name: "subshell wrapping semicolon cd", script: "(cd sub; go test ./...)", dir: "/workspace"},
		{name: "subshell cd then outer command", script: "(cd sub && go test ./...) && echo done", dir: "/workspace"},
		{name: "later cd back", script: "cd sub && go test ./... && cd .. && go vet ./...", dir: "/workspace"},
		{name: "later cd back semicolon", script: "cd sub; go test ./...; cd ..; go vet ./...", dir: "/workspace"},
		{name: "chained cd", script: "cd sub && cd .. && go test ./...", dir: "/workspace"},
		{name: "later pushd", script: "cd sub && pushd other && ls", dir: "/workspace"},
		{name: "relative cd without abs base dir", script: "cd sub && go test ./...", dir: ""},
		{name: "shebang script", script: "#!/bin/bash\ncd sub && go test ./...", dir: "/workspace"},
		{name: "cd with redirect", script: "cd sub >/dev/null && go test ./...", dir: "/workspace"},
		{name: "cd with inline env", script: "CDPATH=/tmp cd sub && go test ./...", dir: "/workspace"},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			gotRest, gotDir, gotMatch := StripLeadingCd(tc.script, tc.dir)
			if gotMatch != tc.wantMatch || gotRest != tc.wantRest || gotDir != tc.wantDir {
				t.Fatalf("StripLeadingCd(%q, %q) = (%q, %q, %v); want (%q, %q, %v)",
					tc.script, tc.dir, gotRest, gotDir, gotMatch, tc.wantRest, tc.wantDir, tc.wantMatch)
			}
		})
	}
}
