package launch

import (
	"context"
	"fmt"
	"path/filepath"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

type directSkillManager struct {
	choices []completion.Choice
	initial map[string]bool
}

func (c *directNativeConnection) openSkillsMenu() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.skillManager = nil
	c.openCommandConfirmationLocked(sessionpresentation.MenuSkills, "Skills", "", []completion.Choice{
		{Name: "list", Display: "List skills"},
		{Name: "manage", Display: "Enable/Disable Skills"},
	})
}

func (c *directNativeConnection) invalidateSkillsLocked() {
	c.skillsGen++
	c.skillsLoaded = false
	c.skills = nil
}

func (c *directNativeConnection) openSkillManager(ctx context.Context, cwd string) error {
	choices, err := c.loadSkillChoices(ctx, cwd)
	if err != nil {
		return fmt.Errorf("list installed skills: %w", err)
	}
	manager := &directSkillManager{initial: make(map[string]bool)}
	for _, skill := range choices {
		if !filepath.IsAbs(skill.Path) {
			continue
		}
		if _, seen := manager.initial[skill.Path]; seen {
			continue
		}
		manager.initial[skill.Path] = skill.Enabled
		manager.choices = append(manager.choices, skill)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(manager.choices) == 0 {
		c.setFeedbackLocked("No skills available.", false)
		c.publishLocked()
		return nil
	}
	c.openCommandConfirmationLocked(sessionpresentation.MenuManageSkills, "Enable/Disable Skills", "", slices.Clone(manager.choices))
	c.skillManager = manager
	return nil
}

// handleSkillsMenuLocked takes ownership of the menu lock from handleMenu.
func (c *directNativeConnection) handleSkillsMenuLocked(ctx context.Context, action sessionpresentation.MenuAction, editor *sessionapi.NativeEditorInput) error {
	menu := c.menu
	if action.Kind == sessionpresentation.MenuDismiss {
		manager := c.skillManager
		c.clearMenuLocked()
		if manager != nil {
			var changes []string
			enabled, disabled := 0, 0
			for _, skill := range manager.choices {
				if manager.initial[skill.Path] == skill.Enabled {
					continue
				}
				if skill.Enabled {
					enabled++
				} else {
					disabled++
				}
			}
			for _, change := range []struct {
				count int
				label string
			}{{enabled, "Enabled"}, {disabled, "Disabled"}} {
				switch change.count {
				case 0:
				case 1:
					changes = append(changes, change.label+" 1 skill")
				default:
					changes = append(changes, fmt.Sprintf("%s %d skills", change.label, change.count))
				}
			}
			if len(changes) > 0 {
				c.setFeedbackLocked(strings.Join(changes, ", "), false)
			}
		}
		c.publishLocked()
		c.mu.Unlock()
		return nil
	}
	if action.Kind == sessionpresentation.MenuSearch && c.skillManager != nil {
		menu.Query = action.Query
		menu.Choices = matchSkillChoices(c.skillManager.choices, action.Query)
		c.publishLocked()
		c.mu.Unlock()
		return nil
	}
	index := slices.IndexFunc(menu.Choices, func(choice completion.Choice) bool {
		return choice.Identity() == action.Choice
	})
	if action.Kind != sessionpresentation.MenuAccept || index < 0 {
		c.mu.Unlock()
		return nil
	}
	choice := menu.Choices[index]
	if menu.Kind == sessionpresentation.MenuSkills {
		c.clearMenuLocked()
		cwd := c.workspace
		if choice.Name == "list" && editor != nil {
			c.effectSeq++
			c.effects = append(c.effects, sessionapi.NativeEffect{
				ID: c.effectSeq, Thread: action.Thread, Target: &editor.Target,
				Action: &composer.Action{Kind: composer.InsertAction, Text: "$"},
			})
		}
		c.publishLocked()
		c.mu.Unlock()
		if choice.Name == "manage" {
			return c.openSkillManager(ctx, cwd)
		}
		return nil
	}

	manager := c.skillManager
	c.mu.Unlock()
	var result struct {
		EffectiveEnabled bool `json:"effectiveEnabled"`
	}
	if err := c.wire.Call(ctx, "skills/config/write", map[string]any{
		"path": choice.Path, "enabled": !choice.Enabled,
	}, &result); err != nil {
		return fmt.Errorf("change skill %s: %w", choice.Label(), err)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.invalidateSkillsLocked()
	if c.skillManager == manager && c.menu == menu {
		for i := range manager.choices {
			if manager.choices[i].Path == choice.Path {
				manager.choices[i].Enabled = result.EffectiveEnabled
				break
			}
		}
		menu.Choices = matchSkillChoices(manager.choices, menu.Query)
	}
	c.publishLocked()
	return nil
}
