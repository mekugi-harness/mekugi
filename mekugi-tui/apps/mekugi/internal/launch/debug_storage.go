package launch

import (
	"errors"
	"os"
	"path/filepath"
)

func stateDirectory() (string, error) {
	base := os.Getenv("XDG_STATE_HOME")
	if base == "" {
		userDirectory, err := os.UserHomeDir()
		if err != nil {
			return "", err
		}
		base = filepath.Join(userDirectory, ".local", "state")
	}
	if !filepath.IsAbs(base) {
		return "", errors.New("XDG_STATE_HOME must be absolute")
	}
	return filepath.Join(base, "mekugi"), nil
}

func debugStorageDirectory() (string, error) {
	base, err := stateDirectory()
	if err != nil {
		return "", err
	}
	return filepath.Join(base, "debug"), nil
}
