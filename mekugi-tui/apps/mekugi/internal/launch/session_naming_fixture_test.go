package launch

import (
	"context"
	"sync"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/inferencegateway"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionnaming"
)

const sessionTitleModel = "gpt-6-luna"

var testNaming = sync.OnceValue(func() *binding.SessionNaming {
	service, err := sessionnaming.Module.New(sessionnaming.Deps{}, module.Slots{})
	if err != nil {
		panic(err)
	}
	registry := new(binding.Registry)
	if err := sessionnaming.Module.Bindings.Register(registry, service, nil); err != nil {
		panic(err)
	}
	return registry.RegisteredNaming()
})

func newTestSessionNaming(ctx context.Context, provider inferencegateway.Forwarder) *sessionNaming {
	return newSessionNaming(ctx, provider, testNaming())
}

func runNamingOperation(ctx context.Context, g *sessionNaming, thread, prompt string) (string, error) {
	entry := testNaming().NewThread("", false)
	entry.ObserveInput(responses.MustMarshalJSON(prompt))
	complete := requestexecution.NamingCompletion(ctx, g.provider, g.usage, thread, sessionTitleAuthHeaders())
	return entry.Admit()(ctx, complete)
}
