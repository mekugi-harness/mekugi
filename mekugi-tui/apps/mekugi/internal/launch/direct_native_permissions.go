package launch

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
)

type directPermissionProfile struct {
	ID string `json:"id"`
}

type directPermissionChange struct {
	profile, policy, reviewer string
}

func (c *directNativeConnection) handlePermissions(ctx context.Context, mainID, args string) error {
	if mainID == "" {
		return errors.New("no active thread")
	}
	if args != "" {
		var change directPermissionChange
		for _, field := range strings.Fields(args) {
			name, value, hasValue := strings.Cut(field, "=")
			switch {
			case !strings.HasPrefix(name, "--") && !hasValue && change.profile == "":
				change.profile = name
			case name == "--approval-policy" && value != "" && change.policy == "":
				change.policy = value
			case name == "--reviewer" && value != "" && change.reviewer == "":
				change.reviewer = value
			default:
				return fmt.Errorf("invalid /permissions argument %q", field)
			}
		}
		return c.applyPermissions(ctx, mainID, change)
	}

	c.mu.Lock()
	cwd := c.workspace
	c.mu.Unlock()
	params := map[string]any{"threadId": mainID}
	var choices []completion.Choice
	for {
		var response struct {
			Data []struct {
				ID          string `json:"id"`
				Description string `json:"description"`
				Allowed     bool   `json:"allowed"`
			} `json:"data"`
			NextCursor string `json:"nextCursor"`
		}
		if err := c.wire.Call(ctx, "permissionProfile/list", params, &response); err != nil {
			return fmt.Errorf("list permission profiles: %w", err)
		}
		for _, profile := range response.Data {
			choices = append(choices, completion.Choice{
				Name: profile.ID, Description: profile.Description, Unavailable: !profile.Allowed,
			})
		}
		if response.NextCursor == "" {
			break
		}
		params["cursor"] = response.NextCursor
	}
	if len(choices) == 0 {
		return errors.New("no permission profiles available")
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if mainID != c.mainThread || cwd != c.workspace {
		return sessionpresentation.ErrStaleIntent
	}
	for i := range choices {
		choices[i].Current = choices[i].Name == c.permissionProfile
	}
	c.openSettingsMenuLocked(mainID, "/permissions", choices)
	return nil
}

func (c *directNativeConnection) isPermissionMenuLocked() bool {
	return c.menu != nil && c.menu.Kind == sessionpresentation.MenuSettings && c.menu.Setting == "/permissions"
}

// handlePermissionMenuLocked retains the chooser if Codex rejects a selection.
// handleMenu hands off the lock on entry.
func (c *directNativeConnection) handlePermissionMenuLocked(ctx context.Context, action sessionpresentation.MenuAction) error {
	menu, thread := c.menu, c.mainThread
	switch action.Kind {
	case sessionpresentation.MenuDismiss:
		c.clearMenuLocked()
		c.publishLocked()
		c.mu.Unlock()
		return nil
	case sessionpresentation.MenuAccept:
		for _, choice := range menu.Choices {
			if choice.Identity() != action.Choice || choice.Unavailable {
				continue
			}
			c.mu.Unlock()
			if err := c.applyPermissions(ctx, thread, directPermissionChange{profile: choice.Name}); err != nil {
				return err
			}
			c.mu.Lock()
			if c.menu == menu {
				c.clearMenuLocked()
				c.publishLocked()
			}
			c.mu.Unlock()
			return nil
		}
		c.mu.Unlock()
		return errors.New("that permission profile is unavailable")
	default:
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
}

func (c *directNativeConnection) applyPermissions(ctx context.Context, mainID string, change directPermissionChange) error {
	params := map[string]any{"threadId": mainID}
	if change.profile != "" {
		params["permissions"] = change.profile
	}
	if change.policy != "" {
		params["approvalPolicy"] = change.policy
	}
	if change.reviewer != "" {
		params["approvalsReviewer"] = change.reviewer
	}
	if err := c.wire.Call(ctx, "thread/settings/update", params, nil); err != nil {
		return fmt.Errorf("update Main's permissions: %w", err)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.mainThread == mainID {
		c.setFeedbackLocked("Main's permissions apply from the next turn", false)
		c.publishLocked()
	}
	return nil
}
