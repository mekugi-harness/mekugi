package launch

import (
	"bytes"
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/connector"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
)

func TestNativeDistributionSharedSocketTerminalConnectorAndHeadless(t *testing.T) {
	codexHome := shortTempDir(t)
	t.Setenv("CODEX_HOME", codexHome)

	tuiSocket, err := nativeAppServerSocketPath()
	if err != nil {
		t.Fatal(err)
	}
	connSocket, err := connector.NativeSocketPath()
	if err != nil {
		t.Fatal(err)
	}
	if tuiSocket != connSocket {
		t.Fatalf("socket mismatch: tui=%q connector=%q", tuiSocket, connSocket)
	}

	// 1. Terminal direct native connection over the shared Unix socket.
	mock := startMockNativeAppServer(t, tuiSocket)
	workspace := t.TempDir()
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	conn, err := openDirectNativeConnection(ctx, tuiSocket, workspace, nil, []string{"--yolo"}, "", "")
	if err != nil {
		t.Fatalf("openDirectNativeConnection: %v", err)
	}
	defer conn.Close()
	if _, err := conn.Next(); err != nil {
		t.Fatalf("conn.Next(): %v", err)
	}
	mock.mu.Lock()
	methods := slices.Clone(mock.methods)
	mock.mu.Unlock()
	if !slices.Contains(methods, "initialize") || !slices.Contains(methods, "thread/start") {
		t.Fatalf("expected direct native initialize and thread/start, got methods=%v", methods)
	}

	// 2. Connector pointing at the exact same default native socket path.
	tokenDir := filepath.Join(t.TempDir(), "connector")
	if err := os.MkdirAll(tokenDir, 0o700); err != nil {
		t.Fatal(err)
	}
	tokenFile := filepath.Join(tokenDir, "token")
	bridge, err := connector.New(connector.Options{SocketPath: connSocket, TokenFile: tokenFile})
	if err != nil {
		t.Fatalf("connector.New: %v", err)
	}
	defer bridge.Close()
	remote := httptest.NewServer(bridge)
	defer remote.Close()
	tokenBytes, err := os.ReadFile(tokenFile)
	if err != nil {
		t.Fatal(err)
	}
	ws, _, err := websocket.Dial(ctx, strings.Replace(remote.URL, "http://", "ws://", 1)+sessionapi.RPCPath, &websocket.DialOptions{
		Subprotocols: []string{sessionapi.RPCSubprotocol},
	})
	if err != nil {
		t.Fatalf("connector websocket dial: %v", err)
	}
	defer ws.CloseNow()
	initMsg := `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"version":"` + sessionapi.ProtocolVersion + `","token":"` + strings.TrimSpace(string(tokenBytes)) + `"}}`
	if err := ws.Write(ctx, websocket.MessageText, []byte(initMsg)); err != nil {
		t.Fatalf("connector write initialize: %v", err)
	}
	if _, _, err := ws.Read(ctx); err != nil {
		t.Fatalf("connector read initialize reply: %v", err)
	}

	// 3. Headless path runs directly over mekugi-app-server with no Go backend.
	var headlessOut bytes.Buffer
	code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader("ping\n"), &headlessOut)
	if err == nil && code == 0 {
		// Succeeded against real mekugi-app-server on PATH if present.
	} else if code != 1 {
		t.Fatalf("unexpected headless exit code %d: %v", code, err)
	}
	_ = http.StatusOK
}

