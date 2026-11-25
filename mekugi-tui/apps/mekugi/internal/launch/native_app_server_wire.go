package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/coder/websocket"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/connector"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
)

// nativeAppServerSocketPath returns the private, user-local Unix socket path
// shared by the direct native mekugi-app-server daemon, terminal UI, and connector.
func nativeAppServerSocketPath() (string, error) {
	return connector.NativeSocketPath()
}

// ensureNativeAppServer coordinates single-instance startup of mekugi-app-server
// on a private Unix domain socket using a separate startup lock and WebSocket
// readiness probe.
func ensureNativeAppServer(ctx context.Context, path string, start func() error) error {
	if !filepath.IsAbs(path) {
		return errors.New("native app-server socket path must be absolute")
	}
	if ready, err := probeNativeAppServer(ctx, path); ready || err != nil {
		return err
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	lock, err := os.OpenFile(path+".startup.lock", os.O_RDWR|os.O_CREATE|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0o600)
	if err != nil {
		return err
	}
	defer func() {
		_ = syscall.Flock(int(lock.Fd()), syscall.LOCK_UN)
		_ = lock.Close()
	}()
	info, err := lock.Stat()
	if err != nil {
		return err
	}
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !info.Mode().IsRegular() || !ok || int(stat.Uid) != os.Geteuid() || info.Mode().Perm()&0o077 != 0 {
		return errors.New("native app-server startup lock must be a private regular file owned by this user")
	}
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			break
		}
		if !errors.Is(err, syscall.EWOULDBLOCK) {
			return err
		}
		if err := waitNativeAppServer(ctx); err != nil {
			return err
		}
	}
	if ready, err := probeNativeAppServer(ctx, path); ready || err != nil {
		return err
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := start(); err != nil {
		return fmt.Errorf("start mekugi-app-server: %w", err)
	}
	for {
		if ready, err := probeNativeAppServer(ctx, path); ready || err != nil {
			return err
		}
		if err := waitNativeAppServer(ctx); err != nil {
			return err
		}
	}
}

func waitNativeAppServer(ctx context.Context) error {
	timer := time.NewTimer(25 * time.Millisecond)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

// probeNativeAppServer verifies a peer-checked WebSocket listener is accepting
// connections on the Unix socket. Only an absent socket or ECONNREFUSED permits
// launching a new daemon.
func probeNativeAppServer(ctx context.Context, path string) (bool, error) {
	if err := ctx.Err(); err != nil {
		return false, err
	}
	info, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	// mekugi-app-server publishes a rendezvous symlink to its private socket, or
	// tests may bind a Unix socket directly at path.
	if info.Mode()&os.ModeSymlink != 0 {
		target, statErr := os.Stat(path)
		if errors.Is(statErr, os.ErrNotExist) {
			return false, nil
		}
		if statErr != nil {
			return false, statErr
		}
		info = target
	}
	if info.Mode()&os.ModeSocket == 0 {
		return false, errors.New("native app-server socket path exists and is not a socket")
	}
	transport := sessionapi.ControlTransport(path)
	defer transport.CloseIdleConnections()
	probeCtx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	conn, _, err := websocket.Dial(probeCtx, "ws://mekugi/", &websocket.DialOptions{
		HTTPClient:      &http.Client{Transport: transport},
		CompressionMode: websocket.CompressionDisabled,
	})
	if err != nil {
		if ctx.Err() != nil {
			return false, context.Cause(ctx)
		}
		if errors.Is(err, os.ErrNotExist) || errors.Is(err, syscall.ECONNREFUSED) {
			return false, nil
		}
		return false, fmt.Errorf("check mekugi-app-server readiness: %w", err)
	}
	_ = conn.Close(websocket.StatusNormalClosure, "")
	return true, nil
}

type nativeAppServerRPCError struct {
	Code    int
	Message string
}

func (e *nativeAppServerRPCError) Error() string {
	if e.Message != "" {
		return e.Message
	}
	return fmt.Sprintf("app-server RPC error %d", e.Code)
}

type nativePendingCall struct {
	done chan appserver.Message
}

// nativeAppServerWire speaks JSON-RPC 2.0 directly over mekugi-app-server's
// Unix-socket WebSocket listener.
type nativeAppServerWire struct {
	ctx       context.Context
	cancel    context.CancelFunc
	transport *http.Transport
	conn      *websocket.Conn
	writeMu   sync.Mutex
	mu        sync.Mutex
	pending   map[string]*nativePendingCall
	nextID    atomic.Uint64
	events    chan appserver.Message
	closed    chan struct{}
	closeOnce sync.Once
	closeErr  error
	userAgent string
}

func dialNativeAppServer(ctx context.Context, socket string) (*nativeAppServerWire, error) {
	transport := sessionapi.ControlTransport(socket)
	conn, _, err := websocket.Dial(ctx, "ws://mekugi/", &websocket.DialOptions{
		HTTPClient:      &http.Client{Transport: transport},
		CompressionMode: websocket.CompressionDisabled,
	})
	if err != nil {
		transport.CloseIdleConnections()
		return nil, fmt.Errorf("connect to mekugi-app-server: %w", err)
	}
	// History snapshots can exceed the default 32 KiB read limit.
	conn.SetReadLimit(64 << 20)
	wireCtx, cancel := context.WithCancel(context.Background())
	w := &nativeAppServerWire{
		ctx:       wireCtx,
		cancel:    cancel,
		transport: transport,
		conn:      conn,
		pending:   make(map[string]*nativePendingCall),
		events:    make(chan appserver.Message, 512),
		closed:    make(chan struct{}),
	}
	go w.readLoop()

	var initResult struct {
		UserAgent string `json:"userAgent"`
	}
	err = w.Call(ctx, "initialize", map[string]any{
		"clientInfo":   map[string]any{"name": "mekugi", "version": mekugiVersion()},
		"capabilities": map[string]any{"experimentalApi": true},
	}, &initResult)
	if err != nil {
		_ = w.Close()
		return nil, fmt.Errorf("initialize mekugi-app-server: %w", err)
	}
	w.userAgent = initResult.UserAgent
	if err := w.Notify(ctx, "initialized", map[string]any{}); err != nil {
		_ = w.Close()
		return nil, fmt.Errorf("send initialized notification: %w", err)
	}
	return w, nil
}

func (w *nativeAppServerWire) Events() <-chan appserver.Message { return w.events }

func (w *nativeAppServerWire) UserAgent() string { return w.userAgent }

func (w *nativeAppServerWire) readLoop() {
	defer func() {
		w.mu.Lock()
		for id, pending := range w.pending {
			close(pending.done)
			delete(w.pending, id)
		}
		w.mu.Unlock()
		close(w.events)
		close(w.closed)
	}()
	for {
		kind, data, err := w.conn.Read(w.ctx)
		if err != nil {
			w.mu.Lock()
			if w.closeErr == nil {
				w.closeErr = err
			}
			w.mu.Unlock()
			return
		}
		if kind != websocket.MessageText {
			continue
		}
		var msg appserver.Message
		if err := json.Unmarshal(data, &msg); err != nil {
			continue
		}
		if msg.Method == "" && len(msg.ID) > 0 {
			key := string(msg.ID)
			w.mu.Lock()
			pending := w.pending[key]
			delete(w.pending, key)
			w.mu.Unlock()
			if pending != nil {
				pending.done <- msg
				close(pending.done)
			}
			continue
		}
		select {
		case w.events <- msg:
		case <-w.ctx.Done():
			return
		}
	}
}

func (w *nativeAppServerWire) writeJSON(ctx context.Context, payload any) error {
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	w.writeMu.Lock()
	defer w.writeMu.Unlock()
	return w.conn.Write(ctx, websocket.MessageText, data)
}

func (w *nativeAppServerWire) Call(ctx context.Context, method string, params any, out any) error {
	idNum := w.nextID.Add(1)
	idRaw := jsontext.Value(fmt.Sprintf("%d", idNum))
	key := string(idRaw)
	var rawParams jsontext.Value
	if params != nil {
		encoded, err := json.Marshal(params)
		if err != nil {
			return err
		}
		rawParams = encoded
	}
	pending := &nativePendingCall{done: make(chan appserver.Message, 1)}
	w.mu.Lock()
	select {
	case <-w.closed:
		w.mu.Unlock()
		return errors.New("mekugi-app-server connection closed")
	default:
		w.pending[key] = pending
	}
	w.mu.Unlock()

	envelope := struct {
		JSONRPC string         `json:"jsonrpc"`
		ID      jsontext.Value `json:"id"`
		Method  string         `json:"method"`
		Params  jsontext.Value `json:"params,omitzero"`
	}{
		JSONRPC: "2.0",
		ID:      idRaw,
		Method:  method,
		Params:  rawParams,
	}
	if err := w.writeJSON(ctx, envelope); err != nil {
		w.mu.Lock()
		delete(w.pending, key)
		w.mu.Unlock()
		return err
	}

	select {
	case reply, ok := <-pending.done:
		if !ok {
			return errors.New("mekugi-app-server connection closed")
		}
		if reply.Error != nil {
			return &nativeAppServerRPCError{Code: reply.Error.Code, Message: reply.Error.Message}
		}
		if out != nil && len(reply.Result) > 0 {
			return json.Unmarshal(reply.Result, out)
		}
		return nil
	case <-ctx.Done():
		w.mu.Lock()
		delete(w.pending, key)
		w.mu.Unlock()
		return ctx.Err()
	}
}

func (w *nativeAppServerWire) Notify(ctx context.Context, method string, params any) error {
	var rawParams jsontext.Value
	if params != nil {
		encoded, err := json.Marshal(params)
		if err != nil {
			return err
		}
		rawParams = encoded
	}
	envelope := struct {
		JSONRPC string         `json:"jsonrpc"`
		Method  string         `json:"method"`
		Params  jsontext.Value `json:"params,omitzero"`
	}{
		JSONRPC: "2.0",
		Method:  method,
		Params:  rawParams,
	}
	return w.writeJSON(ctx, envelope)
}

func (w *nativeAppServerWire) Respond(ctx context.Context, id jsontext.Value, result any) error {
	encoded, err := json.Marshal(result)
	if err != nil {
		return err
	}
	envelope := struct {
		JSONRPC string         `json:"jsonrpc"`
		ID      jsontext.Value `json:"id"`
		Result  jsontext.Value `json:"result"`
	}{
		JSONRPC: "2.0",
		ID:      id,
		Result:  encoded,
	}
	return w.writeJSON(ctx, envelope)
}

func (w *nativeAppServerWire) RespondError(ctx context.Context, id jsontext.Value, code int, message string) error {
	envelope := struct {
		JSONRPC string           `json:"jsonrpc"`
		ID      jsontext.Value   `json:"id"`
		Error   *appserver.Error `json:"error"`
	}{
		JSONRPC: "2.0",
		ID:      id,
		Error:   &appserver.Error{Code: code, Message: message},
	}
	return w.writeJSON(ctx, envelope)
}

func (w *nativeAppServerWire) Close() error {
	var err error
	w.closeOnce.Do(func() {
		w.cancel()
		err = w.conn.Close(websocket.StatusNormalClosure, "")
		w.transport.CloseIdleConnections()
	})
	return err
}
