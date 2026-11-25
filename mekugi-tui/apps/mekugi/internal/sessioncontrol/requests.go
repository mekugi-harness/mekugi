// Package sessioncontrol owns minimal host request correlation and shared
// interrupt/thread types for the headless client and terminal renderer.
package sessioncontrol

import "encoding/json/jsontext"

// Responder preserves the host-owned identity of an admitted server request.
type Responder interface {
	Respond(id jsontext.Value, result any) error
}

// Transport is the app-server's existing request and response surface.
type Transport interface {
	Responder
	Initialize() (string, error)
	Send(method string, params any, request bool) (string, error)
}

// Request retains only routing context. Scope identifies operations retired
// together when their owning session is replaced; Owner stays with the consumer.
type Request[T any] struct {
	Method string
	Scope  string
	Owner  T
}

// Requests is the request ledger for one headless application client.
type Requests[T any] struct {
	transport Transport
	pending   map[string]Request[T]
}

func NewRequests[T any](transport Transport) *Requests[T] {
	return &Requests[T]{transport: transport, pending: make(map[string]Request[T])}
}

func (r *Requests[T]) Initialize(owner T) (string, error) {
	id, err := r.transport.Initialize()
	if err == nil {
		r.pending[id] = Request[T]{Method: "initialize", Owner: owner}
	}
	return id, err
}

func (r *Requests[T]) Send(method string, params any, scope string, owner T) (string, error) {
	id, err := r.transport.Send(method, params, true)
	if err == nil {
		r.pending[id] = Request[T]{Method: method, Scope: scope, Owner: owner}
	}
	return id, err
}

func (r *Requests[T]) Notify(method string, params any) error {
	_, err := r.transport.Send(method, params, false)
	return err
}

func (r *Requests[T]) Lookup(id string) (Request[T], bool) {
	request, ok := r.pending[id]
	return request, ok
}

// Initialized preserves notification-before-request ordering.
func (r *Requests[T]) Initialized(observeShell bool) error {
	if err := r.Notify("initialized", map[string]any{}); err != nil {
		return err
	}
	if observeShell {
		var owner T
		_, err := r.Send("environment/info", map[string]any{"environmentId": "local"}, "main", owner)
		return err
	}
	return nil
}

// Take consumes a response identity once. Unknown or retired identities have no
// owner and must not settle a newer operation with the same method.
func (r *Requests[T]) Take(id string) (Request[T], bool) {
	request, ok := r.pending[id]
	delete(r.pending, id)
	return request, ok
}
