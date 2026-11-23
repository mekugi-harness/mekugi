package changehistory

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/command"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/codexbinding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/internal/retained"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/terminalview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

type Deps struct {
	History *history.Service
	// Directory selects live capture storage; empty enables retained operations only.
	Directory string
}

var Module = module.Descriptor[Deps, *capture.Service, codexbinding.Bindings, terminalui.Definitions[*capture.Service, *scope.Store]]{
	ID: "changehistory", Summary: "Capture, retain, review, apply, and revert filesystem change evidence",
	Requires: []module.ID{"sessionhistory"}, Lifetime: module.Invocation,
	Resources: []string{"private workspace checkpoints and bounded warm-up tasks"},
	Contributes: module.Contributions[*capture.Service]{
		Commands:    []command.Definition[*capture.Service]{{Spec: changes.Command.Spec, Run: (*capture.Service).Execute}},
		RecordKinds: append(retained.RecordKinds(), evidence.RecordKinds()...),
	},
	Views: terminalview.Views,
	New:   newService, Bindings: codexbinding.Codex,
	Start: startService, Stop: stopService,
}
