package launch

import (
	"cmp"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
)

type directRewindTarget struct {
	ThreadID          string   `json:"threadId"`
	TurnID            string   `json:"beforeTurnId"`
	ItemID            string   `json:"beforeItemId,omitempty"`
	ExpectedChangeIDs []string `json:"expectedChangeIds,omitempty"`
}

type directRewindPreview struct {
	ChangeIDs []string `json:"changeIds"`
	Reason    string   `json:"reason"`
}

type directRewindMenu struct {
	thread    string
	next      string
	targets   map[completion.ChoiceID]directRewindTarget
	chosen    directRewindTarget
	changeIDs []string
}

var olderRewindMessages = completion.Choice{Name: "Older messages…"}

func (c *directNativeConnection) openRewind(ctx context.Context) error {
	c.mu.Lock()
	thread := c.mainThread
	if thread == "" || c.busy || c.activeTurn != "" {
		c.mu.Unlock()
		return errors.New("wait for the thread to be idle before rewinding")
	}
	menu := c.menu
	c.mu.Unlock()
	return c.loadRewindPage(ctx, thread, "", menu)
}

func (c *directNativeConnection) loadRewindPage(ctx context.Context, thread, cursor string, previous *sessionpresentation.MenuView) error {
	var page struct {
		Data       []directWireTurn `json:"data"`
		NextCursor string           `json:"nextCursor"`
	}
	// Summary only includes the first user message, omitting steered messages.
	params := map[string]any{"threadId": thread, "sortDirection": "desc", "limit": 20, "itemsView": "full"}
	if cursor != "" {
		params["cursor"] = cursor
	}
	if err := c.wire.Call(ctx, "thread/turns/list", params, &page); err != nil {
		return fmt.Errorf("load user messages for Rewind: %w", err)
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	if c.menu != previous || c.mainThread != thread || c.busy || c.activeTurn != "" {
		return errors.New("the thread changed while loading Rewind")
	}
	rewind := &directRewindMenu{thread: thread, targets: make(map[completion.ChoiceID]directRewindTarget)}
	var choices []completion.Choice
	if cursor != "" {
		rewind = c.rewind
		// Retain all choices already loaded, except the old paging action.
		choices = slices.Clone(previous.Choices[:len(previous.Choices)-1])
	}
	for _, turn := range page.Data {
		if turn.ID == "" || turn.Status == "inProgress" {
			continue
		}
		first := true
		for _, raw := range turn.Items {
			var item struct {
				ID      string         `json:"id"`
				Type    string         `json:"type"`
				Content jsontext.Value `json:"content"`
			}
			if err := json.Unmarshal(raw, &item); err != nil || item.Type != "userMessage" || (!first && item.ID == "") {
				continue
			}
			target := directRewindTarget{ThreadID: thread, TurnID: turn.ID}
			if !first {
				target.ItemID = item.ID
			}
			first = false
			label := strings.Join(strings.Fields(extractUserMessageText(item.Content)), " ")
			choice := completion.Choice{ID: uint64(len(choices) + 1), Name: cmp.Or(label, "User message (no text)")}
			rewind.targets[choice.Identity()] = target
			choices = append(choices, choice)
		}
	}
	rewind.next = page.NextCursor
	if rewind.next != "" {
		choices = append(choices, olderRewindMessages)
	}
	if len(choices) == 0 {
		return errors.New("no user messages available to rewind")
	}
	c.openCommandConfirmationLocked(sessionpresentation.MenuRewind, "Rewind before a user message", "", choices)
	c.rewind = rewind
	return nil
}

func (c *directNativeConnection) acceptRewindLocked(ctx context.Context, choice completion.ChoiceID) error {
	menu, rewind := c.menu, c.rewind
	if rewind == nil || rewind.thread != c.mainThread || c.busy || c.activeTurn != "" {
		c.mu.Unlock()
		return errors.New("wait for the thread to be idle before rewinding")
	}
	if menu.Kind == sessionpresentation.MenuRewindEdits {
		target := rewind.chosen
		switch choice.ID {
		case 1:
		case 2:
			if len(rewind.changeIDs) == 0 {
				c.mu.Unlock()
				return nil
			}
			target.ExpectedChangeIDs = rewind.changeIDs
		default:
			c.mu.Unlock()
			return nil
		}
		c.clearMenuLocked()
		c.publishLocked()
		c.mu.Unlock()
		return c.wire.Call(ctx, "thread/revert", target, nil)
	}
	if choice == olderRewindMessages.Identity() && rewind.next != "" {
		cursor := rewind.next
		c.mu.Unlock()
		return c.loadRewindPage(ctx, rewind.thread, cursor, menu)
	}
	target, found := rewind.targets[choice]
	c.mu.Unlock()
	if !found {
		return nil
	}

	var preview directRewindPreview
	if err := c.wire.Call(ctx, "thread/revert/preview", target, &preview); err != nil {
		preview.Reason = err.Error()
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.menu != menu || c.mainThread != rewind.thread || c.busy || c.activeTurn != "" {
		return nil
	}
	rewind.chosen = target
	rewind.changeIDs = preview.ChangeIDs
	choices := []completion.Choice{{ID: 1, Name: "Keep edits"}}
	if len(preview.ChangeIDs) > 0 {
		choices = append(choices, completion.Choice{
			ID: 2, Name: "Revert edits", Description: fmt.Sprintf("%d captured changes", len(preview.ChangeIDs)), Danger: true,
		})
	}
	c.openCommandConfirmationLocked(sessionpresentation.MenuRewindEdits, "Rewind this thread?", preview.Reason, choices)
	c.rewind = rewind
	return nil
}
