package terminalsession

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
)

func TestResumePathsKeepSuffixAndIdentity(t *testing.T) {
	const path = "/home/dev/projects/very-long-workspace/distinctive-project"
	for _, all := range []bool{false, true} {
		view := sessionpresentation.View{Resume: &sessionpresentation.ResumeView{ID: 1, Workspace: path, All: all, Rows: []sessionpresentation.ResumeRow{{ID: "saved-thread", Workspace: path + "/other-project", Title: "saved title"}}}}
		var picker ResumePicker
		picker.Update(view)
		width := 50
		if all {
			width = 100
		}
		rows := picker.Render(new(terminalui.Painter), width, 20, "")
		want := "distinctive-project"
		if all {
			want = "other-project"
		}
		if text := ansi.Strip(strings.Join(rows, "\n")); !strings.Contains(text, want) || strings.Contains(text, "/home/dev") {
			t.Fatalf("resume path lost suffix: %q", text)
		}
		if action := picker.Key("\r"); action.Thread != "saved-thread" || view.Resume.Workspace != path {
			t.Fatal("painting rebound the resume target")
		}
	}
}

func TestResumePickerDetachedNavigationAndPageDemand(t *testing.T) {
	v := sessionpresentation.View{Version: sessionpresentation.Version{Attachment: 1, Revision: 1}, Resume: &sessionpresentation.ResumeView{
		ID: 1, PageAfter: "next-page", Rows: []sessionpresentation.ResumeRow{{ID: "first"}, {ID: "second"}},
	}}
	var p ResumePicker
	p.Update(v)
	p.Render(new(terminalui.Painter), 80, 30, "")
	p.Key("\x1b[B")
	action := p.Key("\r")
	if action.Kind != sessionpresentation.ResumeChoose || action.Thread != "second" || action.Version != v.Version {
		t.Fatalf("selection: %+v", action)
	}
	page := p.PageDemand()
	if page.Kind != sessionpresentation.ResumePage || page.After != "next-page" {
		t.Fatalf("page demand: %+v", page)
	}
	next := *v.Resume
	next.PageAfter = ""
	next.Rows = append(next.Rows, sessionpresentation.ResumeRow{ID: "third"})
	v.Resume = &next
	v.Version.Revision++
	p.Update(v)
	if p.Selected != 1 || p.Viewport == 0 {
		t.Fatal("page append lost selection or terminal capacity")
	}
	if p.PageDemand().Kind != sessionpresentation.ResumeNoAction {
		t.Fatal("the last page generated another demand")
	}
	p.Update(sessionpresentation.View{})
	if p.Open() {
		t.Fatal("closed publication retained chooser")
	}
}

func TestResumePickerSearchesLoadedAndPagedMetadata(t *testing.T) {
	v := sessionpresentation.View{Resume: &sessionpresentation.ResumeView{
		ID: 1, PageAfter: "next-page", Rows: []sessionpresentation.ResumeRow{{ID: "first", Title: "Unrelated"}},
	}}
	var p ResumePicker
	p.Update(v)
	if effect := p.Key("PARSER"); effect.Kind != sessionpresentation.ResumePage || effect.After != "next-page" {
		t.Fatalf("a search without a loaded match should page: %+v", effect)
	}
	next := *v.Resume
	next.PageAfter = ""
	next.Rows = append(next.Rows,
		sessionpresentation.ResumeRow{ID: "branch-match", Title: "Fix", Branch: "fix/parser"},
		sessionpresentation.ResumeRow{ID: "preview-match", Title: "Issue", Preview: "debug parser"},
		sessionpresentation.ResumeRow{ID: "directory-match", Title: "Test", Workspace: "/tmp/parser"},
		sessionpresentation.ResumeRow{ID: "parser-id", Title: "Cleanup"},
	)
	v.Resume = &next
	p.Update(v)
	for _, id := range []string{"branch-match", "preview-match", "directory-match", "parser-id"} {
		if choice := p.Key("\r"); choice.Kind != sessionpresentation.ResumeChoose || choice.Thread != id {
			t.Fatalf("search selected %+v, want %s", choice, id)
		}
		p.Key("\x1b[B")
	}
	if clear := p.Key("\x1b"); clear.Kind != sessionpresentation.ResumeNoAction ||
		p.query != "" || p.Key("\r").Thread != "parser-id" {
		t.Fatal("Escape did not clear search and keep its selected result")
	}
	if toggle := p.Key("\x1b[C"); toggle.Kind != sessionpresentation.ResumeToggleArchived {
		t.Fatalf("right arrow did not switch Active/Archived: %+v", toggle)
	}
	if close := p.Key("\x1b"); close.Kind != sessionpresentation.ResumeDismiss {
		t.Fatalf("Escape on the unfiltered list did not close the picker: %+v", close)
	}
}
