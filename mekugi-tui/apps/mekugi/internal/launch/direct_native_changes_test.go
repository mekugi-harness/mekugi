package launch

import (
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

func TestDirectNativeSetEditReceiptCollapsesExternalAndEphemeralFiles(t *testing.T) {
	conn := &directNativeConnection{}
	rec := &directItemRecord{
		itemType: "commandExecution",
		entry: activity.Entry{
			Values: []activity.Value{{}},
		},
	}

	files := []nativeCapturedFile{
		{
			path:    "/repo/main.go",
			scope:   "workspace",
			added:   3,
			removed: 1,
			chunk: changes.Chunk{
				Workspace: "/repo",
				Review:    diffmodel.ReviewFile{BeforePath: "/repo/main.go", AfterPath: "/repo/main.go"},
				Origin:    changes.Origin{Change: "amber1", Source: "Shell"},
			},
		},
		{
			path:  "/home/user/.local/state/agent-work/note.md",
			scope: "external",
			chunk: changes.Chunk{
				Workspace: "/repo",
				Review:    diffmodel.ReviewFile{AfterPath: "/home/user/.local/state/agent-work/note.md"},
				Origin:    changes.Origin{Change: "amber1", Source: "Shell"},
			},
		},
		{
			path:  "/tmp/scratch.txt",
			scope: "ephemeral",
			chunk: changes.Chunk{
				Workspace: "/repo",
				Review:    diffmodel.ReviewFile{AfterPath: "/tmp/scratch.txt"},
				Origin:    changes.Origin{Change: "amber1", Source: "Shell"},
			},
		},
	}

	conn.setEditReceiptLocked(rec, files)
	got := rec.entry.Values[0].Changes
	if len(got) != 2 {
		t.Fatalf("expected 1 workspace row + 1 collapsed external summary row, got %d: %+v", len(got), got)
	}
	if got[0].Label != "main.go" || got[0].Added != 3 || got[0].Removed != 1 {
		t.Fatalf("unexpected workspace row: %+v", got[0])
	}
	if got[1].Verb != "Captured" || got[1].Label != "2 external files" || got[1].Note != "mchanges amber1 --summary" {
		t.Fatalf("unexpected collapsed external row: %+v", got[1])
	}
}
