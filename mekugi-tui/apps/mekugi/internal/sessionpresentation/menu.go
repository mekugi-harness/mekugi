package sessionpresentation

import "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"

type MenuKind string

const (
	MenuCompletion   MenuKind = ""
	MenuSkills       MenuKind = "menu"
	MenuManageSkills MenuKind = "manage"
	MenuSettings     MenuKind = "settings"
	MenuArchive      MenuKind = "archive"
	MenuExit         MenuKind = "exit"
	MenuRewind       MenuKind = "rewind"
	MenuRewindEdits  MenuKind = "rewind-edits"
	MenuSideReplace  MenuKind = "side-replace"
	MenuCopy         MenuKind = "copy"
)

type MenuView struct {
	ID   uint64
	Kind MenuKind
	completion.View
	Preferred            completion.ChoiceID
	Saving, QueueCompact bool
	Setting, Explanation string
	Title                string
	Dialog               bool
}
type MenuActionKind uint8

const (
	MenuNoAction MenuActionKind = iota
	MenuDismiss
	MenuAccept
	MenuSearch
)

type MenuAction struct {
	Version Version
	Thread  string `json:",omitzero"`
	Menu    uint64
	Kind    MenuActionKind
	Choice  completion.ChoiceID
	Query   string
}

// MenuState identifies the current application menu; Conversation owns its choices.
// Source: apps/mekugi/internal/router/composer_menu.go:12:16@0ce86dd5.
type MenuState struct {
	Setting   string
	ID        uint64
	Kind      MenuKind
	Preferred completion.ChoiceID
}
