//go:build unix

package launch

import "golang.org/x/sys/unix"

// init marks any inherited outer-tracker descriptors (3, 4, 7, 8) close-on-exec
// so child dash/bash processes spawned by exec_track tests never see occupied
// FDs inherited from an active outer Mekugi inline tracker (MEKUGI-801).
func init() {
	for _, fd := range []uintptr{3, 4, 7, 8} {
		_, _ = unix.FcntlInt(fd, unix.F_SETFD, unix.FD_CLOEXEC)
	}
}
