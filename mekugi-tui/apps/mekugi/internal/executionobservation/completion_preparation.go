package executionobservation

import (
	"errors"
	"fmt"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

type changeReader struct {
	Service changes.Service
	Scope   changes.Scope
}

type capturedSource struct {
	command  string
	stdout   string
	stderr   string
	exitCode int
}

func (s capturedSource) text() string {
	body := s.stdout
	if s.command != "" {
		body = fmt.Sprintf("%s: exit_code=%d\n%s", s.command, s.exitCode, body)
	}
	if s.stderr != "" {
		body += "\n" + s.stderr
	}
	return body
}

func (p *Preparer) completionReader(thread string) (changeReader, error) {
	if p.Retained.Store == nil {
		return changeReader{}, errors.New("completion attachment storage is unavailable")
	}
	selected := p.Retained
	selected.Thread = thread
	admitted, err := selected.Store.ReadAdmittedScope(thread)
	if err != nil {
		return changeReader{}, err
	}
	selected.Namespace = admitted.Namespace
	return changeReader{Service: p.Changes, Scope: selected}, nil
}
