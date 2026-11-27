package launch

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

func (c *directNativeConnection) refreshInputHistory(ctx context.Context, thread string) {
	var response struct {
		Data []struct {
			SessionID string `json:"sessionId"`
			Text      string `json:"text"`
		} `json:"data"`
	}
	err := c.wire.Call(ctx, "thread/inputHistory/read", map[string]string{"threadId": thread}, &response)
	history := make([]composer.HistoryEntry, len(response.Data))
	for i, entry := range response.Data {
		history[i] = composer.HistoryEntry{Source: entry.SessionID, Draft: composer.Draft{Text: entry.Text}, Restored: true}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.mainThread != thread {
		return
	}
	if err != nil {
		c.setFeedbackLocked("Could not read terminal input history: "+err.Error(), true)
	} else {
		c.inputHistory = history
		c.inputHistoryRev++
	}
	c.publishLocked()
}

func (c *directNativeConnection) saveInputHistory(ctx context.Context, text string) error {
	c.mu.Lock()
	thread := c.mainThread
	c.mu.Unlock()
	return c.wire.Call(ctx, "thread/inputHistory/append", map[string]string{"threadId": thread, "text": text}, nil)
}
