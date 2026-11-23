package executionobservation

import (
	"net/http"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/inferencegateway"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
)

func testGatewayResponse(response *http.Response, err error) (*inferencegateway.Response, error) {
	if response == nil {
		return nil, err
	}
	return &inferencegateway.Response{Response: response}, err
}

func testRequestExecutor(provider inferencegateway.Forwarder, issues *requestexecution.Failures, proxy *Preparer) requestexecution.Executor {
	executor := requestexecution.Executor{Provider: provider, Issues: issues}
	if proxy != nil {
		executor.Observations = proxy
		executor.Usage = proxy.Usage
	}
	executor.Exchange, _ = provider.(*inferencegateway.Exchange)
	return executor
}
