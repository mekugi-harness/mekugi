package launch

import (
	"context"
	"crypto/rand"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

type directQueueSubmission struct {
	ID                         string            `json:"id"`
	Kind                       string            `json:"kind"`
	Input                      jsontext.Value    `json:"input"`
	ClientUserMessageID        string            `json:"clientUserMessageId"`
	ResponsesAPIClientMetadata map[string]string `json:"responsesapiClientMetadata"`
}

type directQueueState struct {
	pending  []conversation.PendingText
	waiting  []directQueueSubmission
	revision uint64
}

func (c *directNativeConnection) queueForThreadLocked(threadID string) *directQueueState {
	if threadID != "" && threadID == c.mainThread {
		return &c.mainQueue
	}
	if threadID != "" && threadID == c.sideThread {
		return &c.sideQueue
	}
	return nil
}

func (c *directNativeConnection) refreshQueue(ctx context.Context, threadID string) {
	c.mu.Lock()
	queue := c.queueForThreadLocked(threadID)
	if queue == nil {
		c.mu.Unlock()
		return
	}
	c.queueRead++
	revision := c.queueRead
	queue.revision = revision
	c.mu.Unlock()

	items, err := c.readQueue(ctx, threadID)
	if err != nil {
		return
	}
	pending := make([]conversation.PendingText, 0, len(items))
	waiting := make([]directQueueSubmission, 0, len(items))
	for _, item := range items {
		text := extractUserMessageText(item.Input)
		if text == "" {
			text = "(queued attachment)"
		}
		pending = append(pending, conversation.PendingText{Text: text})
		if item.Kind == "message" {
			waiting = append(waiting, item)
		}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if queue := c.queueForThreadLocked(threadID); queue != nil && queue.revision == revision {
		queue.pending, queue.waiting = pending, waiting
		c.publishLocked()
	}
}

func (c *directNativeConnection) readQueue(ctx context.Context, threadID string) ([]directQueueSubmission, error) {
	// The native queue and its maximum list size are both 100 submissions.
	var response struct {
		Data []directQueueSubmission `json:"data"`
	}
	err := c.wire.Call(ctx, "thread/queue/list", map[string]any{"threadId": threadID, "limit": 100}, &response)
	return response.Data, err
}

func (c *directNativeConnection) editWaiting(ctx context.Context, threadID string) error {
	c.mu.Lock()
	queue := c.queueForThreadLocked(threadID)
	if queue == nil {
		c.mu.Unlock()
		return errors.New("select an input thread to edit waiting messages")
	}
	waiting := slices.Clone(queue.waiting)
	c.mu.Unlock()
	taken, err := c.takeWaiting(ctx, threadID, waiting)
	parts := make([]composer.Draft, 0, len(taken))
	for _, item := range taken {
		parts = append(parts, item.draft)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.restoreInputLocked(threadID, parts)
	return err
}

type directTakenDraft struct {
	id    string
	draft composer.Draft
}

func (c *directNativeConnection) takeWaiting(ctx context.Context, threadID string, items []directQueueSubmission) ([]directTakenDraft, error) {
	ids := make([]string, 0, len(items))
	for _, item := range items {
		if item.Kind == "message" {
			ids = append(ids, item.ID)
		}
	}
	if len(ids) == 0 {
		return nil, nil
	}
	var response struct {
		Data []directQueueSubmission `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/queue/take", map[string]any{
		"threadId": threadID, "queuedSubmissionIds": ids,
	}, &response); err != nil {
		return nil, err
	}
	var decodeErr error
	taken := make([]directTakenDraft, 0, len(response.Data))
	for _, item := range response.Data {
		draft, err := directQueuedDraft(item.Input)
		if err != nil {
			decodeErr = errors.Join(decodeErr, err)
			continue
		}
		taken = append(taken, directTakenDraft{id: item.ID, draft: draft})
	}
	return taken, decodeErr
}

func (c *directNativeConnection) restoreInputLocked(threadID string, parts []composer.Draft) {
	draft := composer.Join(parts...)
	if draft.Text == "" {
		return
	}
	c.effectSeq++
	c.effects = append(c.effects, sessionapi.NativeEffect{ID: c.effectSeq, Thread: threadID, Restore: &draft})
	c.publishLocked()
}

func (c *directNativeConnection) steerWaiting(ctx context.Context, threadID string) error {
	c.mu.Lock()
	queue := c.queueForThreadLocked(threadID)
	if queue == nil {
		c.mu.Unlock()
		return errors.New("select an input thread to steer waiting messages")
	}
	waiting := slices.Clone(queue.waiting)
	turnID := c.activeTurnForLocked(threadID)
	c.mu.Unlock()
	for _, item := range waiting {
		draft, err := directQueuedDraft(item.Input)
		if err != nil {
			return err
		}
		clientID := item.ClientUserMessageID
		if clientID == "" {
			clientID = rand.Text()
		}
		steer := &directSteer{clientID: clientID, queueID: item.ID, draft: draft}
		if err := c.steerInput(ctx, directSteerTurnID{thread: threadID, turn: turnID},
			steer, item.Input, item.ResponsesAPIClientMetadata); err != nil {
			return err
		}
	}
	return nil
}

// Rehydrate native text and its byte ranges. Queue admission snapshots local
// images to data URLs; the editor acquires those bytes through its existing
// retained-effect image path before acknowledging the restore.
func directQueuedDraft(raw jsontext.Value) (composer.Draft, error) {
	var parts []struct {
		Type     string                     `json:"type"`
		Text     string                     `json:"text"`
		Path     string                     `json:"path"`
		URL      string                     `json:"url"`
		Name     string                     `json:"name"`
		Elements []binding.InputTextElement `json:"text_elements"`
	}
	if err := json.Unmarshal(raw, &parts); err != nil {
		return composer.Draft{}, fmt.Errorf("read waiting draft: %w", err)
	}
	var draft composer.Draft
	var previous string
	for _, part := range parts {
		switch part.Type {
		case "text":
			if strings.HasPrefix(part.Text, composer.UserSelectionsPrefix) {
				draft.AnswerSelections += part.Text
				continue
			}
			if previous == "text" && part.Text != "" {
				draft.Text += "\n"
			}
			at := len(draft.Text)
			draft.Text += part.Text
			for _, element := range part.Elements {
				start, end := element.ByteRange.Start, element.ByteRange.End
				if start < 0 || start >= end || end > len(part.Text) {
					continue
				}
				if label := part.Text[start:end]; strings.HasPrefix(label, "@") {
					draft.Files = append(draft.Files, composer.Token{Start: at + start, End: at + end, Path: label[1:]})
				}
			}
		case "image", "localImage":
			path := part.Path
			if part.Type == "image" {
				path = part.URL
			}
			if path != "" {
				start := len(draft.Text)
				draft.Text += fmt.Sprintf("[Image %d]", len(draft.Images)+1)
				draft.Images = append(draft.Images, composer.Token{Start: start, End: len(draft.Text), Path: path})
			}
		case "skill":
			label := "$" + part.Name
			if part.Name == "" {
				continue
			}
			for at := 0; at < len(draft.Text); {
				match := strings.Index(draft.Text[at:], label)
				if match < 0 {
					break
				}
				offset := at + match
				end := offset + len(label)
				if (offset == 0 || strings.ContainsRune(" \t\r\n", rune(draft.Text[offset-1]))) &&
					(end == len(draft.Text) || strings.ContainsRune(" \t\r\n.,;:!?", rune(draft.Text[end]))) {
					draft.Skills = append(draft.Skills, composer.Token{Start: offset, End: end, Path: part.Path, Name: part.Name})
					break
				}
				at = end
			}
		}
		previous = part.Type
	}
	return draft, nil
}
