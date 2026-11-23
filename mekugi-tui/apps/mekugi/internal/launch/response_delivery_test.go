package launch

import (
	"io"
	"net/http"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
)

func copySSETransformed(writer io.Writer, reader io.Reader, transformer codexresponses.Transformer, hooks *requestexecution.ResponseHooks) (responses.TerminalState, error) {
	return requestexecution.CopyResponse(writer, &http.Response{Body: io.NopCloser(reader), Header: http.Header{}}, true, transformer, hooks)
}
