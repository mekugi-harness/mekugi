package launch

import (
	"context"
	"errors"
	"io"
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

type directSteerTurnID struct {
	thread string
	turn   string
}

// This only tracks terminal-local draft recovery. Codex admits, orders and
// delivers steers itself.
type directSteerTurn struct {
	steers   []*directSteer
	inFlight int
	finished bool
	settled  chan struct{}
}

type directSteer struct {
	clientID      string
	queueID       string
	draft         composer.Draft
	accepted      bool
	removedQueued bool
	committed     bool
}

func (c *directNativeConnection) steerTurnLocked(key directSteerTurnID) *directSteerTurn {
	if c.steerTurns == nil {
		c.steerTurns = make(map[directSteerTurnID]*directSteerTurn)
	}
	turn := c.steerTurns[key]
	if turn == nil {
		turn = &directSteerTurn{settled: make(chan struct{})}
		c.steerTurns[key] = turn
	}
	return turn
}

func (c *directNativeConnection) steerInput(ctx context.Context, key directSteerTurnID, steer *directSteer, input any, metadata map[string]string) error {
	c.mu.Lock()
	var turn *directSteerTurn
	if key.turn != "" && key.turn == c.activeTurnForLocked(key.thread) && c.queueForThreadLocked(key.thread) != nil {
		turn = c.steerTurnLocked(key)
		turn.steers = append(turn.steers, steer)
		turn.inFlight++
	}
	c.mu.Unlock()

	params := map[string]any{
		"threadId": key.thread, "expectedTurnId": key.turn, "input": input,
		"clientUserMessageId": steer.clientID,
	}
	if metadata != nil {
		params["responsesapiClientMetadata"] = metadata
	}
	err := c.wire.Call(ctx, "turn/steer", params, nil)
	admitted := err == nil
	removedQueued := steer.queueID == ""
	if admitted && steer.queueID != "" {
		var response struct {
			Deleted bool `json:"deleted"`
		}
		err = c.wire.Call(ctx, "thread/queue/delete", map[string]string{
			"threadId": key.thread, "queuedSubmissionId": steer.queueID,
		}, &response)
		// An already absent row may have been dispatched or taken elsewhere.
		// Only deletion by this client or its own queue/take receipt transfers it.
		removedQueued = err == nil && response.Deleted
	}

	if turn != nil {
		c.mu.Lock()
		turn.inFlight--
		if !admitted {
			turn.steers = slices.DeleteFunc(turn.steers, func(entry *directSteer) bool { return entry == steer })
		}
		steer.accepted = admitted
		steer.removedQueued = steer.removedQueued || removedQueued
		if turn.finished && turn.inFlight == 0 {
			close(turn.settled)
		}
		if !turn.finished && steer.accepted && steer.removedQueued && !steer.committed {
			c.publishLocked()
		}
		c.mu.Unlock()
	}
	return err
}

func (c *directNativeConnection) commitSteerLocked(key directSteerTurnID, clientID string) {
	if turn := c.steerTurns[key]; turn != nil {
		for _, steer := range turn.steers {
			if steer.clientID == clientID {
				steer.committed = true
				steer.draft = composer.Draft{}
			}
		}
	}
}

func (c *directNativeConnection) finishSteerTurnLocked(key directSteerTurnID) {
	if turn := c.steerTurns[key]; turn != nil {
		delete(c.steerTurns, key)
		turn.finished = true
		if turn.inFlight == 0 {
			close(turn.settled)
		}
	}
}

func (c *directNativeConnection) pendingSteersLocked(threadID string) []conversation.PendingText {
	var pending []conversation.PendingText
	if turn := c.steerTurns[directSteerTurnID{thread: threadID, turn: c.activeTurnForLocked(threadID)}]; turn != nil {
		for _, steer := range turn.steers {
			if steer.accepted && steer.removedQueued && !steer.committed {
				pending = append(pending, conversation.PendingText{Text: steer.draft.Text})
			}
		}
	}
	return pending
}

func (c *directNativeConnection) interruptAndRestore(ctx context.Context, key directSteerTurnID, turn *directSteerTurn, params map[string]any) error {
	// Interrupting can dispatch the native queue as soon as the turn ends.
	// Take only the message entries, before making the turn idle.
	items, queueErr := c.readQueue(ctx, key.thread)
	var taken []directTakenDraft
	if queueErr == nil {
		taken, queueErr = c.takeWaiting(ctx, key.thread, items)
	}
	interruptErr := c.wire.Call(ctx, "turn/interrupt", params, nil)
	if interruptErr == nil {
		// The interrupt reply may precede the last commitment notifications, and
		// a steer reply may arrive after turn/completed.
		select {
		case <-turn.settled:
		case <-ctx.Done():
			interruptErr = ctx.Err()
		case <-c.wire.closed:
			interruptErr = io.EOF
		}
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	var parts []composer.Draft
	takenIDs := make(map[string]bool, len(taken))
	for _, item := range taken {
		takenIDs[item.id] = true
	}
	resolved := make(map[string]bool)
	for _, steer := range turn.steers {
		steer.removedQueued = steer.removedQueued || takenIDs[steer.queueID]
		if interruptErr == nil && steer.accepted && steer.removedQueued && !steer.committed {
			parts = append(parts, steer.draft)
		}
		if steer.queueID != "" && (steer.committed || steer.accepted) {
			resolved[steer.queueID] = true
		}
	}
	// A successful take transfers ownership even when the interrupt fails.
	for _, item := range taken {
		if !resolved[item.id] {
			parts = append(parts, item.draft)
		}
	}
	c.restoreInputLocked(key.thread, parts)
	return errors.Join(queueErr, interruptErr)
}
