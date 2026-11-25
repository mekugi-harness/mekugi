package binding

import (
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// MCPServer borrows a feature-owned SDK server and its invocation tool names.
type MCPServer struct {
	Name   string
	Server *mcp.Server
	Tools  []string
}
