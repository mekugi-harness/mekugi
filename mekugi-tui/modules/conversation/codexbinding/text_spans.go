package codexbinding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

type UserContent struct {
	Type     string                     `json:"type"`
	URL      string                     `json:"url"`
	Text     string                     `json:"text"`
	Name     string                     `json:"name"`
	Path     string                     `json:"path"`
	Elements []binding.InputTextElement `json:"text_elements"`
}
