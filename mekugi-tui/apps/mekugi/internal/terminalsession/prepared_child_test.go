package terminalsession

import (
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
)

func TestPreparedChildHumanControls(t *testing.T) {
	call := &prompt.Call{ID: "ask", Thread: "child", Sync: true, Questions: []prompt.Question{{ID: "q", Question: "VISIBLE_CHILD_QUESTION"}}}
	var questions prompt.Questions
	questions.Calls = append(questions.Calls, call)
	questions.Open(call)
	v := sessionpresentation.View{
		Main:               sessionpresentation.Thread{ID: "child", Caller: "/root/worker"},
		Displayed:          sessionpresentation.Thread{ID: "child", Caller: "/root/worker"},
		Admission:          sessionpresentation.InputAdmission{ReadOnly: true},
		Questions:          questions.View("child"),
		InterruptAvailable: true,
	}
	var main Main
	main.Update(v)
	transcript := activityui.NewView()
	prompts := &Prompts{}
	t.Run("render", func(t *testing.T) {
		frame := main.Render(transcript, &transcript.Painter, prompts, MainOptions{Focused: true}, 100, 24, 0)
		if !strings.Contains(strings.Join(frame.Rows, "\n"), "VISIBLE_CHILD_QUESTION") {
			t.Fatal("prepared child published an open question but renderer hid its dock")
		}
	})
	t.Run("escape", func(t *testing.T) {
		if !v.InterruptAvailable {
			t.Fatal("fixture did not publish interrupt availability")
		}
		in := FrameInput{Main: &main, Transcript: transcript, Shell: &Shell{}, Prompts: prompts}
		var state Interaction
		for effect := range in.Dispatch(&state, Input{Kind: KeyInput, Text: "\x1b"}) {
			if effect.Interrupt != nil {
				return
			}
		}
		t.Fatal("prepared child swallowed Escape before issuing available interrupt")
	})
	t.Run("ordinary input", func(t *testing.T) {
		v.Questions.Call, v.Draft.Available = nil, true
		main.Update(v)
		if main.PrepareInput(true).Editable {
			t.Fatal("prepared child exposed ordinary editing")
		}
		v.Admission.ReadOnly = false
		main.Update(v)
		if !main.PrepareInput(true).Editable {
			t.Fatal("transient prompt admission blocked an ordinary root draft")
		}
	})
}
