package launch

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
)

const terminalPreferenceLimit = 4096

// Keep the historical pane record's path and flat schema. Its old inference
// fields are ignored: the native thread owns and restores those settings.
type terminalPreferenceRecord struct {
	Version int `json:"version"`
	terminalsession.Preferences
}

func terminalPreferencePath(workspace, thread string) (string, error) {
	if !filepath.IsAbs(workspace) || thread == "" {
		return "", errors.New("terminal preferences require a workspace and thread")
	}
	base, err := stateDirectory()
	if err != nil {
		return "", err
	}
	key := sha256.Sum256([]byte(workspace + "\x00" + thread))
	return filepath.Join(base, "ui", fmt.Sprintf("panes-%x.json", key)), nil
}

func readTerminalPreferences(workspace, thread string) (*terminalsession.Preferences, error) {
	path, err := terminalPreferencePath(workspace, thread)
	if err != nil {
		return nil, err
	}
	file, err := persistence.OpenRetained(path, terminalPreferenceLimit)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, terminalPreferenceLimit+1))
	if err != nil {
		return nil, err
	}
	if len(data) > terminalPreferenceLimit {
		return nil, errors.New("terminal preference record is too large")
	}
	var record struct {
		Version int `json:"version"`
	}
	if err := json.Unmarshal(data, &record); err != nil {
		return nil, err
	}
	if record.Version != 1 {
		return nil, errors.New("unsupported terminal preference record")
	}
	layout, err := terminalsession.ReadPreferences(data)
	if err != nil {
		return nil, err
	}
	return &layout, nil
}

func (c *directNativeConnection) loadTerminalPreferencesLocked() {
	c.preferences = nil
	layout, err := readTerminalPreferences(c.workspace, c.mainThread)
	if layout != nil {
		c.preferences, err = json.Marshal(layout)
		c.interruptLocked, c.predictionsDisabled = layout.InterruptLocked, layout.PredictionsDisabled
	}
	if err != nil {
		c.setFeedbackLocked(fmt.Sprintf("Could not restore terminal preferences: %v", err), true)
	}
}

func (c *directNativeConnection) saveTerminalPreferences(ctx context.Context, target sessionapi.NativeTarget, data jsontext.Value) error {
	if target.Attachment != c.attachment {
		return sessionpresentation.ErrStaleIntent
	}
	path, err := terminalPreferencePath(target.Workspace, target.Thread)
	if err != nil {
		return err
	}
	layout, err := terminalsession.ReadPreferences(data)
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return err
	}
	current := target.Thread == c.mainThread && target.Workspace == c.workspace
	if current && bytes.Equal(c.preferences, data) {
		return nil
	}
	record, err := json.Marshal(&terminalPreferenceRecord{Version: 1, Preferences: layout})
	if err == nil {
		err = persistence.EnsurePrivateDirectory(filepath.Dir(path))
	}
	if err == nil {
		err = persistence.AtomicFile(path, ".panes-*.tmp", record)
	}
	if err != nil {
		return fmt.Errorf("save terminal preferences: %w", err)
	}
	// A delayed update for the outgoing editor still belongs to that thread,
	// never the replacement thread's live frame.
	if current {
		c.preferences = bytes.Clone(data)
		c.publishLocked()
	}
	return nil
}
