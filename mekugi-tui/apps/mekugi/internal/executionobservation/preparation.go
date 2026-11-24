package executionobservation

import (
	"sync/atomic"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/nativetrace"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	changepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

const applyPatchToolName = "apply_patch"

// Config supplies borrowed invocation owners. Startup owns their lifetimes.
type Config struct {
	ProgressSnapshot func(string) *changepresentation.ReviewView

	Review         *sessionreview.Review
	Audience       *sessioncontrol.Audience
	Windows        *Windows
	NativeTrace    *nativetrace.Reader
	Retained       changes.Scope
	Changes        changes.Service
	Checkpoints    *changecapture.Workspaces
	PublishChanges func([]changes.Update)
	Notice         func(string, string, string, string)
}

// New binds native execution observation to the composed owners without acquiring resources.
func New(config Config) *Preparer {
	return &Preparer{Config: config}
}

type Preparer struct {
	Config
	localShell atomic.Pointer[string]
}

// SetLocalShell records the shell reported by the native local environment.
func (p *Preparer) SetLocalShell(shell string) {
	if p != nil {
		p.localShell.Store(&shell)
	}
}

// LocalShell returns the shell reported by the native local environment.
func (p *Preparer) LocalShell() string {
	if p != nil {
		if shell := p.localShell.Load(); shell != nil {
			return *shell
		}
	}
	return ""
}
