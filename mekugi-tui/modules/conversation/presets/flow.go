package presets

import (
	"fmt"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
)

type Stage string

const (
	Quick   Stage = "presets"
	Edit    Stage = "preset-edit"
	Label   Stage = "preset-label"
	Slots   Stage = "preset-slots"
	Confirm Stage = "preset-confirm"
)

// Flow is view-local. The application owns native config reads and writes.
type Flow struct {
	Stage                       Stage
	Slots                       [10]Slot
	Input, Label, Seed, Problem string
	Slot                        int
	Mode                        string
}

func (f *Flow) View() (title, explanation, query string, choices []completion.Choice) {
	switch f.Stage {
	case Edit:
		title = "New preset"
		if f.Slot >= 0 && f.Slots[f.Slot].Input != "" {
			title = fmt.Sprintf("Edit preset %d", f.Slot)
		}
		return title, "Write a prompt or slash command.", f.Input, []completion.Choice{{Name: "continue"}}
	case Label:
		title = "Label new preset"
		if f.Mode == "rename" {
			title = fmt.Sprintf("Rename preset %d", f.Slot)
		} else if f.Slot >= 0 {
			title = fmt.Sprintf("Label preset %d", f.Slot)
		}
		return title, "Leave empty to show the preset's first line.", f.Label, []completion.Choice{{Name: "continue", Display: "Continue"}}
	case Confirm:
		verb := "Replace"
		explanation = f.Slots[f.Slot].Input + "\n→ " + f.Input
		if f.Mode == "delete" {
			verb = "Delete"
			explanation = f.Slots[f.Slot].Input
		}
		action := fmt.Sprintf("%s preset %d", verb, f.Slot)
		return action + "?", explanation, f.Input, []completion.Choice{{Name: "confirm", Display: action, Danger: true}, {Name: "cancel", Display: "Cancel"}}
	}
	title, explanation = "Presets", "Load a preset into the composer without sending."
	if f.Stage == Slots {
		title = map[string]string{"edit": "Edit a preset", "rename": "Rename a preset", "delete": "Delete a preset", "save": "Save preset to slot"}[f.Mode]
		explanation = "Choose a numbered slot."
	}
	for n := range 10 {
		index := (n + 1) % 10
		value := f.Slots[index]
		if f.Stage == Slots && f.Mode != "save" && value.Input == "" {
			continue
		}
		label := "empty"
		if value.Input != "" {
			label = value.Caption()
		}
		choices = append(choices, completion.Choice{ID: uint64(index + 1), Name: fmt.Sprint(index), Display: fmt.Sprintf("%d  %s", index, label)})
	}
	if f.Stage == Quick {
		choices = append(choices, completion.Choice{Name: "n", Display: "New preset"})
		choices = append(choices,
			completion.Choice{Name: "e", Display: "Edit a preset"},
			completion.Choice{Name: "r", Display: "Rename a preset"},
			completion.Choice{Name: "d", Display: "Delete a preset"},
		)
	}
	return title, explanation, "", choices
}

// Choose advances the workflow or returns the slot to recall or write.
// A zero-value slot to write deletes the selected slot.
func (f *Flow) Choose(choice completion.ChoiceID, query string) (recall string, write *Slot, err error) {
	if choice.Name == "cancel" {
		f.Stage = Quick
		return "", nil, nil
	}
	if choice.ID > 0 {
		f.Slot = int(choice.ID) - 1
	}
	switch f.Stage {
	case Quick:
		switch choice.Name {
		case "n":
			f.Stage, f.Slot, f.Mode, f.Input = Edit, -1, "save", f.Seed
		case "e", "r", "d":
			f.Stage = Slots
			f.Mode = map[string]string{"e": "edit", "r": "rename", "d": "delete"}[choice.Name]
		default:
			if stored := f.Slots[f.Slot]; stored.Input != "" {
				return stored.Input, nil, nil
			}
			f.Stage, f.Mode, f.Input = Edit, "save", f.Seed
		}
	case Edit:
		if err := Validate(query); err != nil {
			return "", nil, err
		}
		f.Stage, f.Input, f.Label = Label, query, ""
		if f.Slot >= 0 {
			f.Label = f.Slots[f.Slot].Label
		}
	case Label:
		label := strings.TrimSpace(query)
		if err := ValidateLabel(label); err != nil {
			return "", nil, err
		}
		f.Label = label
		switch {
		case f.Slot < 0:
			f.Stage = Slots
		case f.Mode != "rename" && f.Slots[f.Slot].Input != "":
			f.Stage = Confirm
		default:
			return "", &Slot{Input: f.Input, Label: f.Label}, nil
		}
	case Slots:
		saved := f.Slots[f.Slot]
		switch f.Mode {
		case "edit":
			f.Stage, f.Input = Edit, saved.Input
		case "rename":
			f.Stage, f.Input, f.Label = Label, saved.Input, saved.Label
		case "delete":
			f.Stage, f.Input, f.Label = Confirm, "", ""
		case "save":
			if saved.Input != "" {
				f.Stage = Confirm
			} else {
				return "", &Slot{Input: f.Input, Label: f.Label}, nil
			}
		}
	case Confirm:
		return "", &Slot{Input: f.Input, Label: f.Label}, nil
	}
	return "", nil, nil
}
