package executionobservation

import (
	"context"

	"io"

	"net/http"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/inferencegateway"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"
)

func executeRequest(
	ctx context.Context,
	executionCtx context.Context,
	request codexresponses.Request,
	headers http.Header,
	sessionID string,
	provider inferencegateway.Forwarder,
	output io.Writer,
	issues *requestexecution.Failures,
	mekugiCalls *Preparer,
) error {
	executor := testRequestExecutor(provider, issues, mekugiCalls)
	executor.Output = output
	return executor.Execute(ctx, executionCtx, request, headers, sessionID)
}
