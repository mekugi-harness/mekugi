// Package presets owns reusable prompt slots and their editing workflow.
package presets

import (
	"errors"
	"strings"
	"unicode/utf8"
)

const Limit = 8000

// Slot is Codex's native tui.presets value. An empty label shows the input's
// first line instead.
type Slot struct {
	Input string `json:"input"`
	Label string `json:"label"`
}

func (s Slot) Caption() string {
	text := strings.TrimSpace(s.Label)
	if text == "" {
		text = strings.TrimSpace(s.Input)
	}
	first, _, _ := strings.Cut(text, "\n")
	return first
}

// Source: codex-rs/config/src/types.rs QuickDial::validate@rust-v0.164.0.
func Validate(input string) error {
	switch {
	case strings.TrimSpace(input) == "":
		return errors.New("Preset input cannot be empty")
	case strings.HasPrefix(strings.TrimSpace(input), "!"):
		return errors.New("Presets support prompts and slash commands, not shell commands")
	case !utf8.ValidString(input):
		return errors.New("Preset input must be valid UTF-8")
	case len(input) > Limit:
		return errors.New("Preset input cannot exceed 8,000 UTF-8 bytes")
	}
	return nil
}

func ValidateLabel(label string) error {
	switch {
	case !utf8.ValidString(label):
		return errors.New("Preset label must be valid UTF-8")
	case strings.ContainsAny(label, "\r\n"):
		return errors.New("Preset label must be a single line")
	}
	return nil
}
