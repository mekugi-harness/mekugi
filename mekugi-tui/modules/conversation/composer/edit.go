package composer

import (
	"errors"
	"slices"
)

func (e *Editor) validRange(start, end int) bool {
	boundary := func(at int) bool {
		return at == 0 || at == len(e.Text) || e.Boundary(e.Boundary(at, -1), 1) == at
	}
	return start >= 0 && end >= start && end <= len(e.Text) && boundary(start) && boundary(end)
}

type ActionKind uint8

const (
	NoAction ActionKind = iota
	InsertAction
	DeleteAction
	MoveAction
	UndoAction
	RedoAction
	RecallPreviousAction
	RecallNextAction
	CompleteFileAction
	CompleteImageAction
	CompleteSkillAction
)

// Action uses source coordinates. Key bindings and wrapped rows belong to the renderer.
type Action struct {
	Kind       ActionKind
	Start, End int
	Text       string
	Run        Run
	Path       string `json:",omitzero"`
}

// Apply preserves the editor's typed-word and deletion grouping.
func (e *Editor) Apply(action Action) (bool, error) {
	before := e.Revision()
	switch action.Kind {
	case NoAction:
	case InsertAction:
		e.Insert(action.Text)
	case DeleteAction:
		if !e.validRange(action.Start, action.End) || action.Run != NoRun && action.Run != BackspaceRun && action.Run != DeleteRun {
			return false, errors.New("Draft deletion must use complete text or attachment boundaries.")
		}
		e.Delete(action.Start, action.End, action.Run)
	case MoveAction:
		if !e.validRange(action.Start, action.Start) {
			return false, errors.New("Draft caret must use complete text or attachment boundaries.")
		}
		e.Run = NoRun
		if e.Cursor() != action.Start {
			e.CursorBack = len(e.Text) - action.Start
			e.Changed()
		}
	case UndoAction, RedoAction:
		e.Undo(action.Kind == RedoAction)
	case RecallPreviousAction, RecallNextAction:
		e.Run = NoRun
		e.Recall(action.Kind == RecallPreviousAction)
	case CompleteFileAction, CompleteImageAction, CompleteSkillAction:
		if !e.validRange(action.Start, action.End) || action.Path == "" {
			return false, errors.New("Attachment completion does not match the draft.")
		}
		draft := e.Draft.Clone()
		draft.Remove(action.Start, action.End)
		label := action.Text
		switch action.Kind {
		case CompleteImageAction:
			label = "[Image]"
		case CompleteSkillAction:
			label = "$" + action.Text
		}
		draft.InsertText(label)
		token := Token{Start: draft.Cursor() - len(label), End: draft.Cursor(), Path: action.Path}
		switch action.Kind {
		case CompleteImageAction:
			draft.Images = append(draft.Images, token)
			draft.RenumberImages()
		case CompleteFileAction:
			draft.Files = append(draft.Files, token)
			slices.SortFunc(draft.Files, func(a, b Token) int { return a.Start - b.Start })
		case CompleteSkillAction:
			token.Name = action.Text
			draft.Skills = append(draft.Skills, token)
			slices.SortFunc(draft.Skills, func(a, b Token) int { return a.Start - b.Start })
		}
		if at := draft.Cursor(); at < len(draft.Text) && draft.Text[at] == ' ' {
			draft.CursorBack--
		} else {
			draft.InsertText(" ")
		}
		e.Record()
		e.Replace(draft)
	default:
		return false, errors.New("Unknown draft action.")
	}
	return e.Revision() != before, nil
}
