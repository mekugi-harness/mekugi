package launch

import (
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

var directSlashCommands = []completion.Choice{
	{Name: "/model", Description: "Set the model (e.g. /model gpt-5)"},
	{Name: "/effort", Description: "Set reasoning effort (low, medium, high, xhigh)"},
	{Name: "/fast", Description: "Toggle fast service tier"},
	{Name: "/compact", Description: "Compact conversation context"},
	{Name: "/clear", Description: "Start a fresh session"},
	{Name: "/new", Description: "Start a fresh session"},
	{Name: "/resume", Description: "Resume a saved session"},
	{Name: "/rewind", Description: "Rewind before a previous turn"},
	{Name: "/archive", Description: "Archive this session"},
	{Name: "/side", Description: "Open or switch to an ephemeral side fork"},
	{Name: "/btw", Description: "Ask a read-only side question"},
	{Name: "/one-off", Description: "Ask a read-only side question"},
	{Name: "/detach", Description: "Detach terminal; keep session running"},
	{Name: "/quit", Description: "Quit the session"},
	{Name: "/exit", Description: "Quit the session"},
}

func slashCompletionTarget(draft composer.Draft) (completion.Target, bool) {
	text := draft.Text
	if !strings.HasPrefix(text, "/") {
		return completion.Target{}, false
	}
	at := len(text) - draft.CursorBack
	if at < 0 {
		at = 0
	}
	if at > len(text) {
		at = len(text)
	}
	if at == 0 || strings.ContainsAny(text[:at], " \t\r\n") {
		return completion.Target{}, false
	}
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

func cloneMenuView(menu *sessionpresentation.MenuView) *sessionpresentation.MenuView {
	if menu == nil {
		return nil
	}
	cp := *menu
	cp.Choices = slices.Clone(menu.Choices)
	return &cp
}

func (c *directNativeConnection) handleCompletion(in sessionapi.NativeEditorInput) {
	target, ok := slashCompletionTarget(in.Draft)
	c.mu.Lock()
	defer c.mu.Unlock()
	if !ok {
		c.dismissedComp = completion.Target{}
		c.completionTarget = conversation.EditorTarget{}
		if c.menu != nil && c.menu.Kind == sessionpresentation.MenuCompletion {
			c.menu = nil
			c.publishLocked()
		}
		return
	}
	if c.dismissedComp == target {
		return
	}
	c.dismissedComp = completion.Target{}
	choices := matchDirectSlashCommands(target.Query)
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

func (c *directNativeConnection) handleMenu(action sessionpresentation.MenuAction, editor *sessionapi.NativeEditorInput) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.menu == nil || (action.Menu != 0 && c.menu.ID != action.Menu) {
		return
	}
	switch action.Kind {
	case sessionpresentation.MenuDismiss:
		if c.menu.Kind == sessionpresentation.MenuCompletion {
			c.dismissedComp = c.menu.Target
		}
		c.menu = nil
		c.completionTarget = conversation.EditorTarget{}
		c.publishLocked()
	case sessionpresentation.MenuAccept:
		menu := c.menu
		c.menu = nil
		c.completionTarget = conversation.EditorTarget{}
		c.dismissedComp = completion.Target{}
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
	}
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
