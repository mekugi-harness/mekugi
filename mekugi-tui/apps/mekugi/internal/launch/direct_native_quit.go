package launch

import (
	"context"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
)

type directQuitTurn struct {
	thread, turn string
}

type directQuitWork struct {
	turns     []directQuitTurn
	processes []sessionpresentation.Process
}

func (c *directNativeConnection) quitWork(ctx context.Context, root string) (directQuitWork, error) {
	var family struct {
		Data []directWireThread `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/subtree/list", map[string]string{"threadId": root}, &family); err != nil {
		return directQuitWork{}, fmt.Errorf("check running threads: %w", err)
	}
	var active []string
	loaded := []string{root}
	for _, thread := range family.Data {
		var status struct {
			Type string `json:"type"`
		}
		if json.Unmarshal(thread.Status, &status) != nil || status.Type == "notLoaded" {
			continue
		}
		if thread.ID != root {
			loaded = append(loaded, thread.ID)
		}
		if status.Type == "active" && !slices.Contains(active, thread.ID) {
			active = append(active, thread.ID)
		}
	}
	var work directQuitWork
	for _, thread := range active {
		var history struct {
			Data []directWireTurn `json:"data"`
		}
		if err := c.wire.Call(ctx, "thread/turns/list", map[string]any{
			"threadId": thread, "limit": 1, "sortDirection": "desc", "itemsView": "notLoaded",
		}, &history); err != nil {
			return directQuitWork{}, fmt.Errorf("read running turn: %w", err)
		}
		if len(history.Data) != 0 && history.Data[0].Status == "inProgress" && history.Data[0].ID != "" {
			work.turns = append(work.turns, directQuitTurn{thread: thread, turn: history.Data[0].ID})
		}
	}
	c.mu.Lock()
	known := make(map[string]bool, len(loaded))
	for _, thread := range loaded {
		known[thread] = c.processLists[thread]
	}
	for _, process := range c.processes {
		if known[process.Thread] {
			work.processes = append(work.processes, process)
		}
	}
	c.mu.Unlock()
	for _, thread := range loaded {
		if known[thread] {
			continue
		}
		listed, err := c.listThreadProcesses(ctx, thread, "")
		if err != nil {
			return directQuitWork{}, fmt.Errorf("check running commands: %w", err)
		}
		work.processes = append(work.processes, listed...)
	}
	return work, nil
}

func (c *directNativeConnection) requestQuit(ctx context.Context, root string) error {
	if root == "" {
		c.finish(&sessionapi.NativeExit{Code: 0})
		return nil
	}
	work, err := c.quitWork(ctx, root)
	if err != nil {
		return err
	}
	c.mu.Lock()
	if root != c.mainThread {
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
	if len(work.turns) > 0 || len(work.processes) > 0 {
		c.offerQuitLocked(work)
		c.mu.Unlock()
		return nil
	}
	c.mu.Unlock()
	c.finish(&sessionapi.NativeExit{Code: 0})
	return nil
}

func (c *directNativeConnection) offerQuitLocked(work directQuitWork) {
	c.openCommandConfirmationLocked(sessionpresentation.MenuExit, "Quit this session?", "Work is still running.", []completion.Choice{
		{ID: 1, Name: "Continue in background"},
		{ID: 2, Name: "Stop", Danger: true},
	})
	c.quitOffer = work
}

func (c *directNativeConnection) stopAndQuit(ctx context.Context, work directQuitWork) error {
	for _, process := range work.processes {
		if _, err := c.terminateThreadProcess(ctx, process); err != nil {
			return err
		}
	}
	for _, target := range work.turns {
		if err := c.wire.Call(ctx, "turn/interrupt", map[string]string{
			"threadId": target.thread, "turnId": target.turn,
		}, nil); err != nil {
			// The turn named in the prompt can finish before Stop reaches the server.
			// Keep the original target; don't look up or interrupt its replacement.
			refusal, ok := errors.AsType[*nativeAppServerRPCError](err)
			if !ok || refusal.Code != -32600 ||
				(refusal.Message != "no active turn to interrupt" &&
					!strings.HasPrefix(refusal.Message, "expected active turn id "+target.turn+" but found ")) {
				return fmt.Errorf("stop running turn: %w", err)
			}
		}
	}
	c.finish(&sessionapi.NativeExit{Code: 0})
	return nil
}
