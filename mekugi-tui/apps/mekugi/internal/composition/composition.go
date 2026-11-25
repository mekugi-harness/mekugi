package composition

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	activityview "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

// Handle resolves one explicitly declared dependency during composition declaration.
type Handle[S any] struct {
	id module.ID
}

type entry struct {
	auxiliary []terminalui.PaneFactory[*scope.Store]
	roster    []terminalui.RosterDefinition[*activityview.View]
	id        module.ID
	requires  []module.ID
}

type Composition struct {
	entries []entry
}

// Add declares a module descriptor for renderer view selection and workspace indexing.
func (c *Composition) Add[D, S any, B interface {
	module.NoBindings | binding.Bindings[S]
}, V interface {
	module.NoViews | terminalui.Definitions[S, *scope.Store] | terminalui.RosterDefinition[*activityview.View]
}](descriptor module.Descriptor[D, S, B, V], _ func() D) *Handle[S] {
	handle := &Handle[S]{id: descriptor.ID}
	node := entry{
		id:       descriptor.ID,
		requires: slices.Clone(descriptor.Requires),
	}
	if roster, ok := any(descriptor.Views).(terminalui.RosterDefinition[*activityview.View]); ok {
		node.roster = []terminalui.RosterDefinition[*activityview.View]{roster}
	}
	views, _ := any(descriptor.Views).(terminalui.Definitions[S, *scope.Store])
	for _, definition := range views.Auxiliary {
		factory := terminalui.PaneFactory[*scope.Store]{Name: definition.Name, Label: definition.Label}
		if definition.Attach != nil {
			factory.Attach = func(theme diffrender.Theme) terminalui.PaneAttachment[*scope.Store] {
				return terminalui.PaneAttachment[*scope.Store]{Pane: definition.Attach(theme).Pane}
			}
		}
		node.auxiliary = append(node.auxiliary, factory)
	}
	c.entries = append(c.entries, node)
	return handle
}

// RendererViews collects descriptor-selected painters without activating a module
// or exposing a source preparation callback to the socket client.
func (c *Composition) RendererViews() (terminalsession.Views, error) {
	var auxiliary []terminalui.PaneFactory[*scope.Store]
	var roster []terminalui.RosterDefinition[*activityview.View]
	for _, node := range c.entries {
		auxiliary = append(auxiliary, node.auxiliary...)
		roster = append(roster, node.roster...)
	}
	return terminalsession.NewViews(auxiliary, roster)
}
