package sessionapi

type rpcMethod struct {
	limit  int
	native bool
}

var rpcMethods = map[string]rpcMethod{
	"session/options":     {limit: 1024},
	"source/skills":       {limit: 1024, native: true},
	"session/list":        {limit: 1024},
	"session/read":        {limit: 1024},
	"session/page":        {limit: 1024},
	"session/start":       {limit: 1024},
	"session/end":         {limit: 1024},
	"session/subscribe":   {limit: 1024},
	"session/input":       {limit: InputBodyLimit},
	"session/shell":       {limit: inputLimit},
	"session/answer":      {limit: InputBodyLimit},
	"session/decide":      {limit: 64 << 10},
	"session/interrupt":   {limit: 1024},
	"session/redraw":      {limit: 1024, native: true},
	"session/control":     {limit: 1024},
	"view/attach":         {limit: 1024},
	"view/intent":         {limit: InputBodyLimit},
	"native/start":        {limit: inputLimit, native: true},
	"subscription/cancel": {limit: 1024},
	"request/cancel":      {limit: 1024},
	"publication/ack":     {limit: 1024},
}
