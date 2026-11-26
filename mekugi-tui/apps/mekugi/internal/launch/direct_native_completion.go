package launch

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

var directSlashCommands = []completion.Choice{
	{Name: "/model", Description: "Choose the model"},
	{Name: "/effort", Description: "Choose the reasoning effort"},
	{Name: "/live", Description: "Show or hide Live edits for this session"},
	{Name: "/tier", Description: "Choose the service tier"},
	{Name: "/fast", Description: "Toggle fast service tier"},
	{Name: "/skills", Description: "List skills"},
	{Name: "/side", Description: "Open or switch to an ephemeral side fork"},
	{Name: "/oneoff", Description: "Run a task in an isolated work-capable fork"},
	{Name: "/btw", Description: "Ask a side question without changing Main"},
	{Name: "/ps", Description: "Show running processes"},
	{Name: "/predictions", Description: "Enable or disable next-message suggestions: on|off"},
	{Name: "/session", Description: "Show session usage and metrics"},
	{Name: "/title", Description: "Rename this session"},
	{Name: "/status", Description: "Show session settings and usage limits"},
	{Name: "/copy", Description: "Copy the last response"},
	{Name: "/rewind", Description: "Rewind before a user turn"},
	{Name: "/fork", Description: "Fork this conversation and switch to the branch"},
	{Name: "/resume", Description: "Resume a saved session"},
	{Name: "/archive", Description: "Archive this session"},
	{Name: "/compact", Description: "Compact context now; Tab queues while busy"},
	{Name: "/clear", Description: "Clear the transcript and start a new session"},
	{Name: "/lock", Description: "Prevent accidental keyboard interruption"},
	{Name: "/unlock", Description: "Allow keyboard interruption again"},
	{Name: "/quit", Description: "Quit the session"},
	{Name: "/detach", Description: "Leave the terminal; keep the session running"},
}

func directCompletionTarget(draft composer.Draft) (completion.Target, bool) {
	text := draft.Text
	at := len(text) - draft.CursorBack
	if at < 0 {
		at = 0
	}
	if at > len(text) {
		at = len(text)
	}
	if at == 0 {
		return completion.Target{}, false
	}
	// Slash command completion only at start of draft with no whitespace before cursor.
	if strings.HasPrefix(text, "/") && !strings.ContainsAny(text[:at], " \t\r\n") {
		end := at
		for end < len(text) && !strings.ContainsAny(text[end:end+1], " \t\r\n") {
			end++
		}
		return completion.Target{
			Kind:  '/',
			Start: 0,
			End:   end,
			Query: strings.TrimPrefix(text[:at], "/"),
		}, true
	}
	// @file and $skill completions at token boundaries.
	start := at
	for start > 0 {
		prev := text[start-1]
		if prev == ' ' || prev == '\t' || prev == '\r' || prev == '\n' {
			break
		}
		start--
	}
	if start >= at {
		return completion.Target{}, false
	}
	sigil := text[start]
	if sigil != '@' && sigil != '$' {
		return completion.Target{}, false
	}
	end := at
	for end < len(text) && !strings.ContainsAny(text[end:end+1], " \t\r\n") {
		end++
	}
	return completion.Target{
		Kind:  sigil,
		Start: start,
		End:   end,
		Query: text[start+1 : at],
	}, true
}

func matchDirectSlashCommands(query string) []completion.Choice {
	q := strings.ToLower(strings.TrimSpace(query))
	if q == "" {
		return slices.Clone(directSlashCommands)
	}
	var prefix, substr []completion.Choice
	for _, cmd := range directSlashCommands {
		name := strings.ToLower(strings.TrimPrefix(cmd.Name, "/"))
		if strings.HasPrefix(name, q) {
			prefix = append(prefix, cmd)
		} else if strings.Contains(name, q) {
			substr = append(substr, cmd)
		}
	}
	return append(prefix, substr...)
}

func matchSkillChoices(skills []completion.Choice, query string) []completion.Choice {
	q := strings.ToLower(strings.TrimSpace(query))
	if q == "" {
		return slices.Clone(skills)
	}
	var prefix, substr []completion.Choice
	for _, sk := range skills {
		name := strings.ToLower(strings.TrimPrefix(sk.Name, "$"))
		desc := strings.ToLower(sk.Description)
		if strings.HasPrefix(name, q) {
			prefix = append(prefix, sk)
		} else if strings.Contains(name, q) || strings.Contains(desc, q) {
			substr = append(substr, sk)
		}
	}
	return append(prefix, substr...)
}

func cloneMenuView(menu *sessionpresentation.MenuView) *sessionpresentation.MenuView {
	if menu == nil {
		return nil
	}
	cp := *menu
	cp.Choices = slices.Clone(menu.Choices)
	return &cp
}

func (c *directNativeConnection) loadSkills(ctx context.Context, cwd string) []completion.Choice {
	c.mu.Lock()
	if c.skillsLoaded && c.skillsCwd == cwd {
		cached := slices.Clone(c.skills)
		c.mu.Unlock()
		return cached
	}
	gen := c.skillsGen
	c.mu.Unlock()
	if c.wire == nil {
		return nil
	}
	params := map[string]any{}
	if cwd != "" {
		params["cwds"] = []string{cwd}
	}
	var catalog binding.SkillCatalog
	if err := c.wire.Call(ctx, "skills/list", params, &catalog); err != nil {
		return nil
	}
	seen := make(map[string]bool)
	var choices []completion.Choice
	for _, bucket := range catalog.Data {
		for _, sk := range bucket.Skills {
			if !sk.Enabled || sk.Name == "" || seen[sk.Name] {
				continue
			}
			seen[sk.Name] = true
			desc := sk.ShortDescription
			if desc == "" && sk.Interface != nil {
				desc = sk.Interface.ShortDescription
			}
			if desc == "" {
				desc = sk.Description
			}
			choices = append(choices, completion.Choice{
				Name:        "$" + sk.Name,
				Path:        sk.Path,
				Display:     "$" + sk.Name,
				Description: desc,
				Enabled:     true,
			})
		}
	}
	c.mu.Lock()
	if c.skillsGen == gen {
		c.skillsCwd = cwd
		c.skillsLoaded = true
		c.skills = slices.Clone(choices)
	}
	c.mu.Unlock()
	return choices
}

func (c *directNativeConnection) searchFiles(ctx context.Context, cwd, query string) []completion.Choice {
	if c.wire == nil || cwd == "" {
		return nil
	}
	var resp struct {
		Files []struct {
			Root     string `json:"root"`
			Path     string `json:"path"`
			FileName string `json:"file_name"`
		} `json:"files"`
	}
	if err := c.wire.Call(ctx, "fuzzyFileSearch", map[string]any{
		"query": query,
		"roots": []string{cwd},
	}, &resp); err != nil {
		return nil
	}
	choices := make([]completion.Choice, 0, len(resp.Files))
	for _, f := range resp.Files {
		if f.Path == "" {
			continue
		}
		choices = append(choices, completion.Choice{
			Name:    "@" + f.Path,
			Path:    f.Path,
			Display: f.Path,
		})
	}
	return choices
}

func (c *directNativeConnection) handleCompletion(ctx context.Context, in sessionapi.NativeEditorInput) {
	target, ok := directCompletionTarget(in.Draft)
	c.mu.Lock()
	if !ok {
		c.dismissedComp = completion.Target{}
		c.completionTarget = conversation.EditorTarget{}
		if c.menu != nil && c.menu.Kind == sessionpresentation.MenuCompletion {
			c.menu = nil
			c.publishLocked()
		}
		c.mu.Unlock()
		return
	}
	if c.dismissedComp == target {
		c.mu.Unlock()
		return
	}
	c.dismissedComp = completion.Target{}
	cwd := c.workspace
	c.mu.Unlock()

	var choices []completion.Choice
	switch target.Kind {
	case '/':
		choices = matchDirectSlashCommands(target.Query)
	case '$':
		choices = matchSkillChoices(c.loadSkills(ctx, cwd), target.Query)
	case '@':
		choices = c.searchFiles(ctx, cwd, target.Query)
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	if len(choices) == 0 {
		c.completionTarget = conversation.EditorTarget{}
		if c.menu != nil && c.menu.Kind == sessionpresentation.MenuCompletion {
			c.menu = nil
			c.publishLocked()
		}
		return
	}
	id := c.menuSeq
	if c.menu == nil || c.menu.Kind != sessionpresentation.MenuCompletion || id == 0 {
		c.menuSeq++
		id = c.menuSeq
	}
	c.completionTarget = in.Target
	c.menu = &sessionpresentation.MenuView{
		ID:           id,
		Kind:         sessionpresentation.MenuCompletion,
		QueueCompact: strings.TrimSpace(in.Draft.Text) == "/compact",
		View: completion.View{
			Target:  target,
			Choices: choices,
			Query:   target.Query,
		},
	}
	c.publishLocked()
}

type directWireModel struct {
	ID                        string `json:"id"`
	Model                     string `json:"model"`
	DisplayName               string `json:"displayName"`
	Description               string `json:"description"`
	Hidden                    bool   `json:"hidden"`
	DefaultReasoningEffort    string `json:"defaultReasoningEffort"`
	SupportedReasoningEfforts []struct {
		ReasoningEffort string `json:"reasoningEffort"`
		Description     string `json:"description"`
	} `json:"supportedReasoningEfforts"`
	ServiceTiers []struct {
		ID          string `json:"id"`
		Name        string `json:"name"`
		DisplayName string `json:"displayName"`
		Description string `json:"description"`
	} `json:"serviceTiers"`
}

func (c *directNativeConnection) listWireModels(ctx context.Context) ([]directWireModel, error) {
	if c.wire == nil {
		return nil, errors.New("not connected")
	}
	c.mu.Lock()
	threadID := c.mainThread
	c.mu.Unlock()
	params := map[string]any{}
	if threadID != "" {
		params["threadId"] = threadID
	}
	var resp struct {
		Data []directWireModel `json:"data"`
	}
	if err := c.wire.Call(ctx, "model/list", params, &resp); err != nil {
		return nil, fmt.Errorf("list models: %w", err)
	}
	return resp.Data, nil
}

func (c *directNativeConnection) openSettingsMenuLocked(setting string, choices []completion.Choice) {
	c.menuSeq++
	c.completionTarget = conversation.EditorTarget{}
	c.dismissedComp = completion.Target{}
	var preferred completion.ChoiceID
	for _, ch := range choices {
		if ch.Current {
			preferred = ch.Identity()
			break
		}
	}
	c.menu = &sessionpresentation.MenuView{
		ID:        c.menuSeq,
		Kind:      sessionpresentation.MenuSettings,
		Setting:   setting,
		Preferred: preferred,
		View: completion.View{
			Choices: choices,
		},
	}
	c.publishLocked()
}

func (c *directNativeConnection) openModelMenu(ctx context.Context) error {
	models, err := c.listWireModels(ctx)
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	choices := make([]completion.Choice, 0, len(models))
	for _, m := range models {
		if m.Hidden {
			continue
		}
		name := cmp.Or(m.Model, m.ID)
		if name == "" {
			continue
		}
		choices = append(choices, completion.Choice{
			Name:        name,
			Display:     cmp.Or(m.DisplayName, name),
			Description: m.Description,
			Current:     c.model != "" && (m.Model == c.model || m.ID == c.model),
		})
	}
	if len(choices) == 0 {
		return errors.New("no models available")
	}
	c.openSettingsMenuLocked("/model", choices)
	return nil
}

func (c *directNativeConnection) openEffortMenu(ctx context.Context) error {
	models, _ := c.listWireModels(ctx)
	c.mu.Lock()
	defer c.mu.Unlock()
	var choices []completion.Choice
	if m, ok := findWireModel(models, c.model); ok {
		for _, opt := range m.SupportedReasoningEfforts {
			if opt.ReasoningEffort == "" {
				continue
			}
			choices = append(choices, completion.Choice{
				Name:        opt.ReasoningEffort,
				Description: opt.Description,
				Current:     c.effort == opt.ReasoningEffort,
			})
		}
	}
	if len(choices) == 0 {
		for _, level := range []string{"low", "medium", "high", "xhigh"} {
			choices = append(choices, completion.Choice{
				Name:    level,
				Current: c.effort == level,
			})
		}
	}
	c.openSettingsMenuLocked("/effort", choices)
	return nil
}

func (c *directNativeConnection) openTierMenu(ctx context.Context) error {
	models, _ := c.listWireModels(ctx)
	c.mu.Lock()
	defer c.mu.Unlock()
	var choices []completion.Choice
	hasDefault := false
	m, foundModel := findWireModel(models, c.model)
	if foundModel {
		for _, tier := range m.ServiceTiers {
			if tier.ID == "" {
				continue
			}
			lower := strings.ToLower(tier.ID)
			isStandard := lower == "standard" || lower == "default"
			if isStandard {
				hasDefault = true
			}
			choices = append(choices, completion.Choice{
				Name:        tier.ID,
				Display:     cmp.Or(tier.Name, tier.DisplayName, tier.ID),
				Description: tier.Description,
				Current:     c.tier == tier.ID || (isStandard && (c.tier == "" || c.tier == "standard" || c.tier == "default")),
			})
		}
	}
	if !hasDefault {
		defaultChoice := completion.Choice{
			Name:        "default",
			Display:     "Standard",
			Description: "Default service tier",
			Current:     c.tier == "" || c.tier == "standard" || c.tier == "default",
		}
		choices = append([]completion.Choice{defaultChoice}, choices...)
	}
	if !foundModel && len(choices) == 1 {
		choices = []completion.Choice{
			choices[0],
			{
				Name:        "fast",
				Description: "Faster responses at priority pricing",
				Current:     c.tier == "fast" || c.tier == "priority",
			},
		}
	}
	c.openSettingsMenuLocked("/tier", choices)
	return nil
}

func findWireModel(models []directWireModel, current string) (directWireModel, bool) {
	if current == "" {
		return directWireModel{}, false
	}
	for _, m := range models {
		if m.Model == current || m.ID == current {
			return m, true
		}
	}
	return directWireModel{}, false
}

func (c *directNativeConnection) applyServiceTier(ctx context.Context, threadID, tier string) error {
	lower := strings.ToLower(strings.TrimSpace(tier))
	var value any = strings.TrimSpace(tier)
	if lower == "" || lower == "standard" || lower == "default" || lower == "none" {
		value = nil
	}
	return c.wire.Call(ctx, "thread/settings/update", map[string]any{
		"threadId":    threadID,
		"serviceTier": value,
	}, nil)
}

func (c *directNativeConnection) handleMenu(ctx context.Context, action sessionpresentation.MenuAction, editor *sessionapi.NativeEditorInput) error {
	c.mu.Lock()
	if c.menu == nil || (action.Menu != 0 && c.menu.ID != action.Menu) {
		c.mu.Unlock()
		return nil
	}
	switch action.Kind {
	case sessionpresentation.MenuDismiss:
		if c.menu.Kind == sessionpresentation.MenuCompletion {
			c.dismissedComp = c.menu.Target
		}
		c.menu = nil
		c.completionTarget = conversation.EditorTarget{}
		c.publishLocked()
		c.mu.Unlock()
		return nil
	case sessionpresentation.MenuAccept:
		menu := c.menu
		mainID := c.mainThread
		c.menu = nil
		c.completionTarget = conversation.EditorTarget{}
		c.dismissedComp = completion.Target{}
		if menu.Kind == sessionpresentation.MenuSettings {
			c.publishLocked()
			c.mu.Unlock()
			if mainID == "" || action.Choice.Name == "" {
				return nil
			}
			switch menu.Setting {
			case "/model":
				return c.wire.Call(ctx, "thread/settings/update", map[string]any{
					"threadId": mainID,
					"model":    action.Choice.Name,
				}, nil)
			case "/effort":
				return c.wire.Call(ctx, "thread/settings/update", map[string]any{
					"threadId": mainID,
					"effort":   action.Choice.Name,
				}, nil)
			case "/tier":
				return c.applyServiceTier(ctx, mainID, action.Choice.Name)
			}
			return nil
		}
		if menu.Kind == sessionpresentation.MenuCompletion && editor != nil && action.Choice.Name != "" {
			target := editor.Target
			start, end := menu.Target.Start, menu.Target.End
			if start < 0 {
				start = 0
			}
			if end > len(editor.Draft.Text) {
				end = len(editor.Draft.Text)
			}
			if end > start {
				c.effectSeq++
				t := target
				c.effects = append(c.effects, sessionapi.NativeEffect{
					ID:     c.effectSeq,
					Thread: action.Thread,
					Target: &t,
					Action: &composer.Action{
						Kind:  composer.DeleteAction,
						Start: start,
						End:   end,
					},
				})
				target.Revision++
			}
			c.effectSeq++
			t := target
			c.effects = append(c.effects, sessionapi.NativeEffect{
				ID:     c.effectSeq,
				Thread: action.Thread,
				Target: &t,
				Action: &composer.Action{
					Kind: composer.InsertAction,
					Text: action.Choice.Name,
				},
			})
		}
		c.publishLocked()
		c.mu.Unlock()
		return nil
	}
	c.mu.Unlock()
	return nil
}

func (c *directNativeConnection) handleReceipt(receipt sessionapi.NativeReceipt) {
	if receipt.Effect == 0 {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.effects = slices.DeleteFunc(c.effects, func(effect sessionapi.NativeEffect) bool {
		return effect.ID <= receipt.Effect
	})
}
