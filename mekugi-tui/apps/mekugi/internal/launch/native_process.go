package launch

import (
	"context"
	"os"
	"os/exec"
	"syscall"
)

// ensureNativeServer starts one detached native mekugi-app-server daemon
// listening on unix://PATH only when the peer-checked endpoint is absent.
// Waiting belongs to the client; execution does not.
func ensureNativeServer(ctx context.Context, socket string) error {
	return ensureNativeAppServer(ctx, socket, func() error {
		executable, err := invocationExecutable("mekugi-app-server", os.Environ(), "")
		if err != nil {
			return err
		}
		command := exec.Command(executable, "--listen", "unix://"+socket)
		command.Env = serverEnvironment(os.Environ())
		command.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
		// Nil streams attach to the null device. The server never inherits the
		// client's terminal, descriptors, foreground process group or cancellation.
		if err := command.Start(); err != nil {
			return err
		}
		go func() { _ = command.Wait() }()
		return nil
	})
}
