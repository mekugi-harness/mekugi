package sessionapi

import (
	"context"
)

// NativeSkillsSocketEnvironment passes the actual server endpoint to its
// workers, including invocations using a nondefault control socket.
const NativeSkillsSocketEnvironment = "MEKUGI_CONTROL_SOCKET"

type nativeSkillCatalog struct {
	Paths   map[string][]string `json:"paths"`
	Problem string              `json:"problem,omitempty"`
}

func (h *Hub) nativeSkillCatalog(ctx context.Context, thread string) (nativeSkillCatalog, *RPCError) {
	runtime, err := h.Requests(thread)
	if err != nil {
		return nativeSkillCatalog{}, rpcFailure(409, err.Error())
	}
	reader, ok := runtime.(interface {
		NativeSkillCatalog(context.Context, string) (map[string][]string, error)
	})
	if !ok {
		// Backends without a native catalog keep their managed skill reader.
		return nativeSkillCatalog{Paths: map[string][]string{}}, nil
	}
	paths, err := reader.NativeSkillCatalog(ctx, thread)
	result := nativeSkillCatalog{Paths: paths}
	if err != nil {
		result.Problem = err.Error()
	}
	return result, nil
}
