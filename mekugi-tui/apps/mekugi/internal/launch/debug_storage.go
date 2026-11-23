package launch

import (
	"path/filepath"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func debugStorageDirectory() (string, error) {
	base, err := history.StateDirectory()
	if err != nil {
		return "", err
	}
	return filepath.Join(base, "debug"), nil
}
