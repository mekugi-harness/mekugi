package launch

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"strconv"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presets"
)

func (c *directNativeConnection) readPresets(ctx context.Context, thread string) ([10]presets.Slot, error) {
	var response struct {
		Presets map[string]presets.Slot `json:"presets"`
	}
	if err := c.wire.Call(ctx, "thread/presets/list", map[string]any{"threadId": thread}, &response); err != nil {
		return [10]presets.Slot{}, err
	}
	var slots [10]presets.Slot
	for i := range slots {
		slots[i] = response.Presets[strconv.Itoa(i)]
	}
	return slots, nil
}

func (c *directNativeConnection) presetsAvailableLocked() bool {
	return c.mainThread != "" && c.mainThread == c.inputThread && c.mainThread == c.displayedThread
}

func (c *directNativeConnection) openPresetMenu(ctx context.Context, seed string) error {
	c.mu.Lock()
	if !c.presetsAvailableLocked() {
		c.mu.Unlock()
		return errors.New("open presets from Main")
	}
	thread := c.mainThread
	c.mu.Unlock()
	slots, err := c.readPresets(ctx, thread)
	if err != nil {
		return fmt.Errorf("read presets: %w", err)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.mainThread != thread || !c.presetsAvailableLocked() {
		return sessionpresentation.ErrStaleIntent
	}
	c.clearMenuLocked()
	c.presetCatalog = new(slots)
	c.presetFlow = &presets.Flow{Stage: presets.Quick, Slots: slots, Slot: -1, Seed: seed}
	c.dismissedComp = completion.Target{}
	c.publishPresetMenuLocked(true)
	return nil
}

func (c *directNativeConnection) publishPresetMenuLocked(newStage bool) {
	if newStage {
		c.menuSeq++
	}
	f := c.presetFlow
	title, explanation, query, choices := f.View()
	c.menu = &sessionpresentation.MenuView{
		ID: c.menuSeq, Kind: sessionpresentation.MenuKind(f.Stage), Dialog: true,
		Title: title, Explanation: explanation,
		View: completion.View{Choices: choices, Query: query, Problem: f.Problem},
	}
	if f.Stage == presets.Confirm {
		c.menu.Preferred = completion.ChoiceID{Name: "cancel"}
	}
	c.publishLocked()
}

// handlePresetMenuLocked receives the menu lock from handleMenu and releases it
// before any RPC, so input rendering continues during a configuration write.
func (c *directNativeConnection) handlePresetMenuLocked(ctx context.Context, action sessionpresentation.MenuAction) error {
	f := c.presetFlow
	if action.Kind == sessionpresentation.MenuDismiss {
		c.clearMenuLocked()
		c.publishLocked()
		c.mu.Unlock()
		return nil
	}
	if !c.presetsAvailableLocked() {
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
	if action.Kind == sessionpresentation.MenuSearch && f.Stage == presets.Label {
		f.Label, f.Problem = action.Query, ""
		c.publishPresetMenuLocked(false)
		c.mu.Unlock()
		return nil
	}
	if action.Kind != sessionpresentation.MenuAccept || !slices.ContainsFunc(c.menu.Choices, func(choice completion.Choice) bool {
		return choice.Identity() == action.Choice
	}) {
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
	stage, thread := f.Stage, c.mainThread
	f.Problem = ""
	recall, write, err := f.Choose(action.Choice, action.Query)
	if err != nil {
		f.Problem = err.Error()
		c.publishPresetMenuLocked(false)
		c.mu.Unlock()
		return err
	}
	if recall != "" {
		c.clearMenuLocked()
		c.effectSeq++
		c.effects = append(c.effects, sessionapi.NativeEffect{ID: c.effectSeq, Thread: thread, Recall: &recall})
		c.publishLocked()
		c.mu.Unlock()
		return nil
	}
	if write == nil {
		c.publishPresetMenuLocked(f.Stage != stage)
		c.mu.Unlock()
		return nil
	}
	index, slots := f.Slot, f.Slots
	c.mu.Unlock()
	saved, err := c.writePreset(ctx, thread, index, slots[index], *write)
	c.mu.Lock()
	defer c.mu.Unlock()
	if err == nil && c.mainThread == thread && c.presetCatalog != nil {
		slots = *c.presetCatalog
		slots[index] = saved
		c.presetCatalog = new(slots)
	}
	if c.presetFlow != f {
		return err
	}
	if err != nil {
		f.Problem = err.Error()
		c.publishPresetMenuLocked(false)
		return err
	}
	f.Slots, f.Stage = slots, presets.Quick
	verb := "Saved"
	if write.Input == "" {
		verb = "Deleted"
	}
	c.setFeedbackLocked(fmt.Sprintf("%s preset %d", verb, index), false)
	c.publishPresetMenuLocked(true)
	return nil
}

func (c *directNativeConnection) writePreset(ctx context.Context, thread string, index int, before, after presets.Slot) (presets.Slot, error) {
	var expected, replacement *presets.Slot
	if before.Input != "" {
		expected = &before
	}
	if after.Input != "" {
		replacement = &after
	}
	var response struct {
		Preset presets.Slot `json:"preset"`
	}
	if err := c.wire.Call(ctx, "thread/presets/write", map[string]any{
		"threadId": thread, "slot": index, "expected": expected, "preset": replacement,
	}, &response); err != nil {
		return presets.Slot{}, fmt.Errorf("update preset %d: %w", index, err)
	}
	return response.Preset, nil
}

func (c *directNativeConnection) namedPresets(ctx context.Context, query string) []completion.Choice {
	c.mu.Lock()
	if !c.presetsAvailableLocked() {
		c.mu.Unlock()
		return nil
	}
	thread, catalog := c.mainThread, c.presetCatalog
	c.mu.Unlock()
	if catalog == nil {
		slots, err := c.readPresets(ctx, thread)
		if err != nil {
			return nil // Keep skill completion available if preset discovery fails.
		}
		c.mu.Lock()
		if c.mainThread != thread || !c.presetsAvailableLocked() {
			c.mu.Unlock()
			return nil
		}
		if c.presetCatalog == nil {
			c.presetCatalog = new(slots)
		}
		catalog = c.presetCatalog
		c.mu.Unlock()
	}
	var choices []completion.Choice
	query = strings.ToLower(query)
	for n := range 10 {
		index := (n + 1) % 10
		value := catalog[index]
		if value.Input == "" || strings.TrimSpace(value.Label) == "" {
			continue
		}
		label := value.Caption()
		if strings.Contains(strings.ToLower(label), query) {
			preview, _, _ := strings.Cut(strings.TrimSpace(value.Input), "\n")
			choices = append(choices, completion.Choice{
				ID: uint64(index + 1), Name: label,
				Description: fmt.Sprintf("Preset %d: %s", index, preview), Preset: true,
			})
		}
	}
	return choices
}
