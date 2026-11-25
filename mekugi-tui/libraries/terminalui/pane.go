package terminalui

import (
	"context"
	"io"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

// Pane receives prepared values; painting and input have no source capabilities.
type Pane interface {
	Paint(context.Context, io.Writer, int, int) error
	Title() (string, string)
	Hints() Hints
	Key(string) bool
	Mouse(byte, int, int)
	ClearHover()
	SetTheme(diffrender.Theme)
	Invalidate()
	Reset()
}

type PaneSelection interface {
	Selection(int, int) SelectionSource
}
type PaneNavigator interface {
	NavigatorWidth(int) int
	NavigatorColumns() int
	SetNavigatorColumns(int)
}
type PaneEditor interface {
	EditorPath() string
	Workspace() string
}

// SelectionSource freezes pane-local source geometry and its complete document.
// Source: modules/changehistory/terminalview/pane_selection.go:8:20@d367500b
type SelectionSource struct {
	X, Y, Width int
	Painted     []diffrender.LineSource
	Document    []string
	Sources     []diffrender.LineSource
	Offset      int
	Fixed       int
	Workspace   string
}

type PaneFactory[C any] struct {
	Name, Label string
	Attach      func(diffrender.Theme) PaneAttachment[C]
}

// PaneAttachment separates application publication from renderer interaction.
// Only the application retains Prepare; it runs before delivering the pane.
type PaneAttachment[C any] struct {
	Pane    Pane
	Prepare func(C)
}

// RosterDefinition selects a feature-owned roster painter. Its concrete value
// type stays checked by composition without a dependency on feature packages.
type RosterDefinition[V any] struct {
	Name, Label string
	New         func() V
}
