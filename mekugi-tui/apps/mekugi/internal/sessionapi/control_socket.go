package sessionapi

import (
	"context"
	"errors"
	"net"
	"net/http"
	"os"
	"path/filepath"
)

const (
	ProtocolHeader  = "Mekugi-Protocol"
	ProtocolVersion = "9"
	ResourcePath    = "/api/resources/"
)

// ControlTransport connects directly to a same-user server, without a network
// proxy. Callers supply ProtocolHeader and close their transport when finished.
func ControlTransport(path string) *http.Transport {
	return &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			if !filepath.IsAbs(path) {
				return nil, errors.New("server control socket path must be absolute")
			}
			if err := private(filepath.Dir(path)); err != nil {
				return nil, err
			}
			dialPath := path
			if info, err := os.Lstat(path); err == nil && info.Mode()&os.ModeSymlink != 0 {
				resolved, err := filepath.EvalSymlinks(path)
				if err != nil {
					return nil, err
				}
				if !filepath.IsAbs(resolved) {
					return nil, errors.New("server control socket target must be absolute")
				}
				if err := private(filepath.Dir(resolved)); err != nil {
					return nil, err
				}
				dialPath = resolved
			}
			var dialer net.Dialer
			conn, err := dialer.DialContext(ctx, "unix", dialPath)
			if err != nil {
				return nil, err
			}
			if err := ownPeer(conn); err != nil {
				_ = conn.Close()
				return nil, err
			}
			return conn, nil
		},
	}
}
