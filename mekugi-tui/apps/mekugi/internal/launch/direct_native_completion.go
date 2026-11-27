package launch

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"unicode"

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
	{Name: "/reasoning", Description: "Choose the reasoning effort"},
	{Name: "/permissions", Description: "Choose Main's permission profile"},
	{Name: "/presets", Description: "Recall or edit a numbered prompt preset"},
	{Name: "/live", Description: "Show or hide Live edits for this session"},
	{Name: "/tier", Description: "Choose the service tier"},
	{Name: "/skills", Description: "List or manage skills"},
	{Name: "/oneoff", Description: "Run a task in an isolated work-capable fork"},
	{Name: "/btw", Description: "Ask a side question without changing Main"},
	{Name: "/ps", Description: "Show running processes"},
	{Name: "/predictions", Description: "Toggle next-message suggestions"},
	{Name: "/session", Description: "Show session usage and metrics"},
	{Name: "/title", Description: "Rename this session"},
	{Name: "/status", Description: "Show session settings and usage limits"},
	{Name: "/copy", Description: "Copy the last response"},
	{Name: "/rewind", Description: "Rewind before a user turn"},
	{Name: "/fork", Description: "Fork this conversation, optionally with a prompt"},
	{Name: "/resume", Description: "Resume a saved session"},
	{Name: "/archive", Description: "Archive this session"},
	{Name: "/compact", Description: "Compact context now; Tab queues while busy"},
	{Name: "/clear", Description: "Clear the transcript and start a new thread"},
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
	if strings.HasPrefix(text, "/") {
		end := strings.IndexFunc(text, unicode.IsSpace)
		if end < 0 {
			end = len(text)
		}
		if at <= end {
			return completion.Target{Kind: '/', End: end, Query: text[1:end]}, true
		}
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
	if sigil == '$' && slices.ContainsFunc(draft.Skills, func(skill composer.Token) bool {
		return skill.Start == start && skill.End <= end
	}) {
		return completion.Target{}, false
	}
	return completion.Target{
		Kind:  sigil,
		Start: start,
		End:   end,
		Query: text[start+1 : at],
	}, true
}

func matchDirectSlashCommands(query string) []completion.Choice {
	if query == "" {
		return slices.Clone(directSlashCommands)
	}
	needle := []rune(strings.ToLower(query))
	type match struct {
		choice completion.Choice
		score  int
	}
	var matches []match
	for _, cmd := range directSlashCommands {
		first, matched := -1, 0
		for i, letter := range cmd.Name[1:] {
			if letter != needle[matched] {
				continue
			}
			if first == -1 {
				first = i
			}
			matched++
			if matched == len(needle) {
				score := i - first + 1 - len(needle)
				if first == 0 {
					score -= 100
				}
				matches = append(matches, match{cmd, score})
				break
			}
		}
	}
	slices.SortStableFunc(matches, func(a, b match) int { return cmp.Compare(a.score, b.score) })
	choices := make([]completion.Choice, 0, len(matches))
	for _, match := range matches {
		choices = append(choices, match.choice)
	}
	return choices
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
	catalog, _ := c.loadSkillChoices(ctx, cwd)
	seen := make(map[string]bool)
	var choices []completion.Choice
	for _, skill := range catalog {
		if !skill.Enabled || seen[skill.Name] {
			continue
		}
		seen[skill.Name] = true
		skill.Display = skill.Name
		choices = append(choices, skill)
	}
	return choices
}

func (c *directNativeConnection) loadSkillChoices(ctx context.Context, cwd string) ([]completion.Choice, error) {
	c.mu.Lock()
	if c.skillsLoaded && c.skillsCwd == cwd {
		cached := slices.Clone(c.skills)
		c.mu.Unlock()
		return cached, nil
	}
	gen := c.skillsGen
	c.mu.Unlock()
	if c.wire == nil {
		return nil, errors.New("not connected")
	}
	params := map[string]any{}
	if cwd != "" {
		params["cwds"] = []string{cwd}
	}
	var catalog binding.SkillCatalog
	if err := c.wire.Call(ctx, "skills/list", params, &catalog); err != nil {
		return nil, err
	}
	var choices []completion.Choice
	for _, bucket := range catalog.Data {
		if cwd != "" && bucket.Cwd != cwd {
			continue
		}
		for _, sk := range bucket.Skills {
			if sk.Name == "" {
				continue
			}
			desc := sk.ShortDescription
			label := sk.Name
			if desc == "" && sk.Interface != nil {
				desc = sk.Interface.ShortDescription
			}
			if sk.Interface != nil && sk.Interface.DisplayName != "" {
				label = sk.Interface.DisplayName
			}
			if desc == "" {
				desc = sk.Description
			}
			choices = append(choices, completion.Choice{
				Name:        "$" + sk.Name,
				Path:        sk.Path,
				Display:     label,
				Description: desc,
				Enabled:     sk.Enabled,
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
	return choices, nil
}

func (c *directNativeConnection) searchFiles(ctx context.Context, threadID, query string) []completion.Choice {
	if c.wire == nil || threadID == "" {
		return nil
	}
	var resp struct {
		Paths []string `json:"paths"`
	}
	if err := c.wire.Call(ctx, "thread/files/search", map[string]any{
		"threadId": threadID,
		"query":    query,
	}, &resp); err != nil {
		return nil
	}
	choices := make([]completion.Choice, 0, len(resp.Paths))
	for _, path := range resp.Paths {
		if path == "" {
			continue
		}
		choices = append(choices, completion.Choice{
			Name:    "@" + path,
			Path:    path,
			Display: path,
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
			c.clearMenuLocked()
			c.publishLocked()
		}
		c.mu.Unlock()
		return
	}
	if c.dismissedComp == target {
		c.mu.Unlock()
		return
	}
	if c.menu != nil && c.menu.Kind != sessionpresentation.MenuCompletion {
		c.mu.Unlock()
		return
	}
	c.dismissedComp = completion.Target{}
	cwd := c.workspace
	threadID := cmp.Or(c.inputThread, c.mainThread)
	c.mu.Unlock()

	var choices []completion.Choice
	switch target.Kind {
	case '/':
		choices = matchDirectSlashCommands(target.Query)
	case '$':
		choices = matchSkillChoices(c.loadSkills(ctx, cwd), target.Query)
		if in.Target.Call == "" && in.Target.Request == "" {
			choices = append(choices, c.namedPresets(ctx, target.Query)...)
		}
	case '@':
		choices = c.searchFiles(ctx, threadID, target.Query)
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	if c.menu != nil && c.menu.Kind != sessionpresentation.MenuCompletion {
		return
	}
	if target.Kind == '@' && threadID != cmp.Or(c.inputThread, c.mainThread) {
		return
	}
	if len(choices) == 0 {
		c.completionTarget = conversation.EditorTarget{}
		if c.menu != nil && c.menu.Kind == sessionpresentation.MenuCompletion {
			c.clearMenuLocked()
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

func (c *directNativeConnection) listWireModels(ctx context.Context, threadID string) ([]directWireModel, error) {
	if c.wire == nil {
		return nil, errors.New("not connected")
	}
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

func (c *directNativeConnection) openSettingsMenuLocked(threadID, setting string, choices []completion.Choice) {
	c.clearMenuLocked()
	c.settingsMenuThread = threadID
	c.menuSeq++
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
		Dialog:    setting == "/permissions",
		Preferred: preferred,
		View: completion.View{
			Choices: choices,
		},
	}
	c.publishLocked()
}

func (c *directNativeConnection) selectedModelSettings() (threadID, model, effort string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.sideThread != "" && c.inputThread == c.sideThread {
		return c.sideThread, c.sideModel, c.sideEffort
	}
	return c.mainThread, c.model, c.effort
}

func (c *directNativeConnection) openModelMenu(ctx context.Context, threadID, current string) error {
	models, err := c.listWireModels(ctx, threadID)
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
			Current:     current != "" && (m.Model == current || m.ID == current),
		})
	}
	if len(choices) == 0 {
		return errors.New("no models available")
	}
	c.openSettingsMenuLocked(threadID, "/model", choices)
	return nil
}

func (c *directNativeConnection) openEffortMenu(ctx context.Context, threadID, model, current string) error {
	models, err := c.listWireModels(ctx, threadID)
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	var choices []completion.Choice
	if m, ok := findWireModel(models, model); ok {
		for _, opt := range m.SupportedReasoningEfforts {
			if opt.ReasoningEffort == "" {
				continue
			}
			choices = append(choices, completion.Choice{
				Name:        opt.ReasoningEffort,
				Description: opt.Description,
				Current:     current == opt.ReasoningEffort,
			})
		}
	}
	if len(choices) == 0 {
		return errors.New("No advertised reasoning effort choices; use /effort VALUE")
	}
	c.openSettingsMenuLocked(threadID, "/effort", choices)
	return nil
}

func (c *directNativeConnection) openTierMenu(ctx context.Context, mainID string) error {
	models, _ := c.listWireModels(ctx, mainID)
	c.mu.Lock()
	defer c.mu.Unlock()
	current := c.tier
	if directServiceTierValue(current) == nil {
		current = "default"
	}
	choices := []completion.Choice{{
		Name:        "default",
		Display:     "Default",
		Description: "Default service tier",
		Current:     current == "default",
	}}
	m, foundModel := findWireModel(models, c.model)
	if foundModel {
		for _, tier := range m.ServiceTiers {
			if directServiceTierValue(tier.ID) == nil {
				continue
			}
			choices = append(choices, completion.Choice{
				Name:        tier.ID,
				Display:     cmp.Or(tier.Name, tier.DisplayName, tier.ID),
				Description: tier.Description,
				Current:     current == tier.ID,
			})
		}
	}
	if !slices.ContainsFunc(choices, func(choice completion.Choice) bool { return choice.Current }) {
		choices = append(choices, completion.Choice{Name: current, Description: "Current", Current: true})
	}
	c.openSettingsMenuLocked(mainID, "/tier", choices)
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
	return c.wire.Call(ctx, "thread/settings/update", map[string]any{
		"threadId":    threadID,
		"serviceTier": directServiceTierValue(tier),
	}, nil)
}

func directServiceTierValue(tier string) any {
	tier = strings.TrimSpace(tier)
	switch strings.ToLower(tier) {
	case "", "standard", "default", "none":
		return nil
	default:
		return tier
	}
}

func (c *directNativeConnection) clearMenuLocked() {
	c.menu = nil
	c.presetFlow = nil
	c.sideReplaceTask = ""
	c.settingsMenuThread = ""
	c.completionTarget = conversation.EditorTarget{}
	c.copyChoices = nil
	c.rewind = nil
	c.skillManager = nil
	c.quitOffer = directQuitWork{}
}

func (c *directNativeConnection) openCommandConfirmationLocked(kind sessionpresentation.MenuKind, title, explanation string, choices []completion.Choice) {
	c.clearMenuLocked()
	c.menuSeq++
	c.menu = &sessionpresentation.MenuView{
		ID: c.menuSeq, Kind: kind, Title: title, Explanation: explanation,
		View: completion.View{Choices: choices},
	}
	c.publishLocked()
}

func (c *directNativeConnection) handleMenu(ctx context.Context, action sessionpresentation.MenuAction, editor *sessionapi.NativeEditorInput) error {
	c.mu.Lock()
	if c.menu == nil || (action.Menu != 0 && c.menu.ID != action.Menu) {
		c.mu.Unlock()
		return nil
	}
	if c.presetFlow != nil {
		return c.handlePresetMenuLocked(ctx, action)
	}
	if c.isPermissionMenuLocked() {
		return c.handlePermissionMenuLocked(ctx, action)
	}
	if c.menu.Kind == sessionpresentation.MenuSkills || c.menu.Kind == sessionpresentation.MenuManageSkills {
		return c.handleSkillsMenuLocked(ctx, action, editor)
	}
	if action.Kind == sessionpresentation.MenuAccept && (c.menu.Kind == sessionpresentation.MenuRewind || c.menu.Kind == sessionpresentation.MenuRewindEdits) {
		return c.acceptRewindLocked(ctx, action.Choice)
	}
	switch action.Kind {
	case sessionpresentation.MenuDismiss:
		if c.menu.Kind == sessionpresentation.MenuCompletion {
			c.dismissedComp = c.menu.Target
		}
		c.clearMenuLocked()
		c.publishLocked()
		c.mu.Unlock()
		return nil
	case sessionpresentation.MenuAccept:
		menu := c.menu
		completionEditor := c.completionTarget
		mainID := c.mainThread
		quitOffer := c.quitOffer
		settingsThread := c.settingsMenuThread
		sideTask := c.sideReplaceTask
		if menu.Kind == sessionpresentation.MenuCopy {
			c.copyMenuChoiceLocked(action.Choice)
		}
		c.clearMenuLocked()
		c.dismissedComp = completion.Target{}
		if menu.Kind == sessionpresentation.MenuSideReplace {
			c.publishLocked()
			c.mu.Unlock()
			if action.Choice != menu.Choices[1].Identity() {
				return nil
			}
			if err := c.handleCloseSide(ctx); err != nil {
				return err
			}
			return c.startSide(ctx, sideTask)
		}
		if menu.Kind == sessionpresentation.MenuArchive {
			c.publishLocked()
			c.mu.Unlock()
			if action.Choice != menu.Choices[1].Identity() || mainID == "" {
				return nil
			}
			if err := c.wire.Call(ctx, "thread/archive", map[string]any{"threadId": mainID}, nil); err != nil {
				return err
			}
			c.mu.Lock()
			if c.mainThread == mainID {
				c.archived = true
				c.publishLocked()
			}
			c.mu.Unlock()
			return nil
		}
		if menu.Kind == sessionpresentation.MenuExit {
			c.publishLocked()
			c.mu.Unlock()
			switch action.Choice {
			case menu.Choices[0].Identity():
				c.finish(&sessionapi.NativeExit{Code: 0})
			case menu.Choices[1].Identity():
				return c.stopAndQuit(ctx, quitOffer)
			}
			return nil
		}
		if menu.Kind == sessionpresentation.MenuSettings {
			c.publishLocked()
			c.mu.Unlock()
			if mainID == "" || action.Choice.Name == "" {
				return nil
			}
			switch menu.Setting {
			case "/model":
				return c.wire.Call(ctx, "thread/settings/update", map[string]any{
					"threadId": settingsThread,
					"model":    action.Choice.Name,
				}, nil)
			case "/effort":
				return c.wire.Call(ctx, "thread/settings/update", map[string]any{
					"threadId": settingsThread,
					"effort":   action.Choice.Name,
				}, nil)
			case "/tier":
				return c.applyServiceTier(ctx, mainID, action.Choice.Name)
			}
			return nil
		}
		if menu.Kind == sessionpresentation.MenuCompletion && editor != nil && action.Choice.Name != "" {
			target := editor.Target
			text := action.Choice.Name
			if menu.Target.Kind == '/' {
				text += " "
			}
			if menu.Target.Kind == '@' {
				cwd := c.workspace
				c.mu.Unlock()
				return c.completePath(menu, action, *editor, cwd)
			}
			selected := slices.IndexFunc(menu.Choices, func(choice completion.Choice) bool { return choice.Identity() == action.Choice })
			if menu.Target.Kind == '$' && (selected < 0 || editor.Target != completionEditor) {
				c.publishLocked()
				c.mu.Unlock()
				return sessionpresentation.ErrStaleIntent
			}
			if selected >= 0 && menu.Choices[selected].Preset {
				if !c.presetsAvailableLocked() || c.presetCatalog == nil {
					c.publishLocked()
					c.mu.Unlock()
					return sessionpresentation.ErrStaleIntent
				}
				text = c.presetCatalog[action.Choice.ID-1].Input
			} else if menu.Target.Kind == '$' {
				c.effectSeq++
				c.effects = append(c.effects, sessionapi.NativeEffect{
					ID: c.effectSeq, Thread: action.Thread, Target: &target,
					Action: &composer.Action{
						Kind: composer.CompleteSkillAction, Start: menu.Target.Start, End: menu.Target.End,
						Text: strings.TrimPrefix(text, "$"), Path: menu.Choices[selected].Path,
					},
				})
				c.publishLocked()
				c.mu.Unlock()
				return nil
			}
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
					Text: text,
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

// Source: mekugi-tui/apps/mekugi/internal/launch/app_server_picker.go:335:372@cc041cd0 acceptCompletion.
func (c *directNativeConnection) completePath(menu *sessionpresentation.MenuView, action sessionpresentation.MenuAction, editor sessionapi.NativeEditorInput, cwd string) error {
	choice := slices.IndexFunc(menu.Choices, func(choice completion.Choice) bool { return choice.Identity() == action.Choice })
	target, ok := directCompletionTarget(editor.Draft)
	if !ok || target != menu.Target || choice < 0 {
		c.mu.Lock()
		c.publishLocked()
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
	path := menu.Choices[choice].Path
	absolutePath := path
	if !filepath.IsAbs(absolutePath) {
		absolutePath = filepath.Join(cwd, absolutePath)
	}
	edit := composer.Action{Kind: composer.CompleteFileAction, Start: target.Start, End: target.End, Path: path}
	if imageFile(absolutePath) {
		edit.Kind, edit.Path = composer.CompleteImageAction, absolutePath
	} else {
		if strings.ContainsFunc(path, unicode.IsSpace) && !strings.Contains(path, `"`) {
			path = `"` + path + `"`
		}
		edit.Text = "@" + path
	}
	c.mu.Lock()
	c.effectSeq++
	c.effects = append(c.effects, sessionapi.NativeEffect{
		ID: c.effectSeq, Thread: action.Thread, Target: &editor.Target, Action: &edit,
	})
	c.publishLocked()
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
