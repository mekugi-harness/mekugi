package execscope

import "testing"

func TestSessionScratchRedirectsDoesNotGuessNewBindings(t *testing.T) {
	for _, command := range []string{
		`SESSION_TMP=/elsewhere; cat > "$SESSION_TMP/file"`,
		`read SESSION_TMP; cat > "$SESSION_TMP/file"`,
		`echo "${other:=x}"; cat > "$SESSION_TMP/file"`,
		`cat > "$SESSION_TMP/../outside"`,
		`cat > "${SESSION_TMP:-/elsewhere}/file"`,
		`cat > '$SESSION_TMP/file'`,
	} {
		if paths := SessionScratchRedirects(command, "bash", "/tmp/session"); len(paths) != 0 {
			t.Errorf("guessed paths for %q: %v", command, paths)
		}
	}
}
