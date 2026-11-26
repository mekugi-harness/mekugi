package launch

import (
	"fmt"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/dlclark/regexp2/v2"
)

func listenPrivateUnix(path string) (net.Listener, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	_ = os.Remove(path)
	l, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	_ = os.Chmod(path, 0o600)
	return l, nil
}

func TestMain(m *testing.M) {
	if os.Getenv("MEKUGI_NATIVE_SERVER_FIXTURE") != "" && len(os.Args) > 1 && os.Args[1] == "--listen" {
		runNativeServerFixture()
	}
	temporary, err := filepath.Abs(os.TempDir())
	if err == nil {
		temporary, err = filepath.EvalSymlinks(temporary)
	}
	if err == nil {
		err = os.Setenv("TMPDIR", temporary)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "resolve test temporary directory: %v\n", err)
		os.Exit(1)
	}
	var stateDirectory string
	if os.Getenv("MEKUGI_LAUNCHER_TEST_STATE_ISOLATED") == "" {
		stateDirectory, err = os.MkdirTemp("", "mekugi-test-state-")
		if err == nil {
			err = os.Setenv("XDG_STATE_HOME", stateDirectory)
		}
		if err == nil {
			err = os.Setenv("XDG_CONFIG_HOME", filepath.Join(stateDirectory, "config"))
		}
		if err == nil {
			err = os.Setenv("MEKUGI_LAUNCHER_TEST_STATE_ISOLATED", "1")
		}
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
	clock := regexp2.MustCompile("x", regexp2.None)
	clock.MatchTimeout = 1000 * time.Hour
	if _, err := clock.MatchString("x"); err != nil {
		fmt.Fprintln(os.Stderr, "start regexp2 timeout clock:", err)
		os.Exit(1)
	}
	if err := os.Unsetenv(axReadOutputEnvironment); err != nil {
		fmt.Fprintln(os.Stderr, "isolate AX test instrumentation:", err)
		os.Exit(1)
	}
	code := m.Run()
	if stateDirectory != "" {
		if err := os.RemoveAll(stateDirectory); err != nil {
			_, _ = fmt.Fprintf(os.Stderr, "clean up launch test state: %v\n", err)
			code = 1
		}
	}
	os.Exit(code)
}
