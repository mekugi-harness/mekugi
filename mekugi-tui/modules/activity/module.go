package activity

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

var Module = module.Descriptor[Deps, *feed.Service, module.NoBindings, terminalui.RosterDefinition[*terminalview.View]]{
	ID: "activity", Summary: "Own host-operation feeds, roster derivation and retained output",
	Lifetime: module.AttachedSession, New: newService,
	Views: terminalui.RosterDefinition[*terminalview.View]{Name: "activity", Label: "Agents", New: terminalview.NewView},
	// Historical display snapshots retain ordinary expiry without live native waits.
	// Source: modules/activity/waittargets/record.go@[bd4147b31^] Record.
	Contributes: module.Contributions[*feed.Service]{RecordKinds: []recordkind.Spec{{ID: "wait", Match: recordkind.HashedName("wait-", ".json")}}},
}
