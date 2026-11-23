package conversation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/codexbinding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

var Module = module.Descriptor[Deps, *presentation.Service, codexbinding.Bindings, module.NoViews]{
	ID: "conversation", Summary: "Own conversation drafts, transcripts, prompts, queues, and side answers",
	Lifetime: module.AttachedSession, New: newService, Bindings: codexbinding.Codex,
}
