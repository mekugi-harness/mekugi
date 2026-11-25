package sessionapi

import (
	"context"
	"fmt"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

type attachRetainFixture struct {
	workspace   string
	mainConn    *NativeConnection
	mainEvent   NativeEvent
	childReview *changes.ReviewView
	childFamily *Family
}

func newAttachRetainFixture(tb testing.TB, review *changes.ReviewView, family *Family, mainHist, childHist *activity.Publication) *attachRetainFixture {
	tb.Helper()
	workspace := tb.TempDir()
	review.Workspace = workspace
	for i := range review.Files {
		for j := range review.Files[i].Chunks {
			review.Files[i].Chunks[j].Workspace = workspace
		}
	}
	f := &attachRetainFixture{workspace: workspace, childReview: review, childFamily: family}
	mainServer, childServer := NewLive(), NewLive()
	_ = mainServer.SetSummary(Summary{Thread: "main", Workspace: workspace, Ready: true})
	_ = childServer.SetSummary(Summary{Thread: "child", Workspace: workspace, Ready: true})

	hub := NewHub(tb.Context(), nil)
	tb.Cleanup(hub.Close)
	hub.mu.Lock()
	hub.sessions["main"] = &listedSession{entry: listed{Thread: "main", Workspace: workspace}, server: mainServer}
	hub.sessions["child"] = &listedSession{entry: listed{Thread: "child", Workspace: workspace, ParentThread: "main"}, server: childServer}
	hub.mu.Unlock()

	ctx, cancel := context.WithCancel(tb.Context())
	tb.Cleanup(cancel)
	serve := func(s *Server, id, caller string, hist *activity.Publication, rev func() *changes.ReviewView, fam func() *Family) {
		go func() {
			for {
				select {
				case <-ctx.Done():
					return
				case req := <-s.Native().Requests():
					thread := sessionpresentation.Thread{ID: id, Caller: caller}
					frame := NativeFrame{
						View: sessionpresentation.View{
							Version: sessionpresentation.Version{Attachment: 1, Revision: 1},
							Main:    thread, Displayed: thread, Input: thread,
							Info: sessionpresentation.MainInfo{Workspace: workspace, Caller: caller},
						},
						Main:   hist,
						Review: rev(),
						Family: fam(),
					}
					if err := s.Native().Publish(req.Attachment, frame); err != nil {
						return
					}
					req.Reply <- ""
				}
			}
		}()
	}
	serve(mainServer, "main", "/root", mainHist, func() *changes.ReviewView { return review }, func() *Family { return family })
	serve(childServer, "child", "/root/worker", childHist, func() *changes.ReviewView { return f.childReview }, func() *Family { return f.childFamily })

	httpServer := httptest.NewServer(Versioned(hub.Handler()))
	tb.Cleanup(httpServer.Close)
	var err error
	f.mainConn, err = OpenNative(tb.Context(), httpServer.Client(), httpServer.URL, "main")
	if err != nil {
		tb.Fatal(err)
	}
	tb.Cleanup(func() { _ = f.mainConn.Close() })
	f.mainEvent, err = f.mainConn.Next()
	if err != nil {
		tb.Fatal(err)
	}
	return f
}

func TestNativeAttachRetainsFamilyAndReviewAcrossSameConnection(t *testing.T) {
	review := &changes.ReviewView{
		Revision: 7,
		Publication: changes.Publication{
			Files: []changes.File{{Path: "large.go", Chunks: []changes.Chunk{{Key: "chunk-1", Caller: "/root", Review: diffmodel.ReviewFile{Diff: strings.Repeat("+saved diff line\n", 4096)}}}}},
		},
	}
	family := &Family{
		Root: "main", Revision: 3, Threads: []string{"main", "child"},
		Agents: &activity.Publication{MainSummary: []activity.Entry{{Seq: 1, Kind: "agent", Text: strings.Repeat("summary ", 2048)}}},
	}
	f := newAttachRetainFixture(t, review, family,
		&activity.Publication{Entries: []activity.Entry{{Seq: 1, Kind: "assistant", Text: "main transcript"}}, LastSeq: 1},
		&activity.Publication{Entries: []activity.Entry{{Seq: 1, Kind: "assistant", Text: "child transcript"}}, LastSeq: 1},
	)

	// Same Review and Family: child's initial snapshot reuses both pointers.
	childConn, err := f.mainConn.Attach(t.Context(), "child")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = childConn.Close() })
	childEvent, err := childConn.Next()
	if err != nil {
		t.Fatal(err)
	}
	if childEvent.Frame.Review != f.mainEvent.Frame.Review || childEvent.Frame.Family != f.mainEvent.Frame.Family {
		t.Fatal("child initial snapshot did not retain identical Review and Family across same-connection attach")
	}
	if len(childEvent.Frame.Main.Entries) != 1 || childEvent.Frame.Main.Entries[0].Text != "child transcript" {
		t.Fatalf("unexpected child main transcript: %+v", childEvent.Frame.Main)
	}

	// Updated Review and Family: child's initial snapshot delivers the new owners.
	updatedReview, updatedFamily := new(*review), new(*family)
	updatedReview.Revision, updatedFamily.Revision = 8, 4
	f.childReview, f.childFamily = updatedReview, updatedFamily

	childConn2, err := f.mainConn.Attach(t.Context(), "child")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = childConn2.Close() })
	childEvent2, err := childConn2.Next()
	if err != nil {
		t.Fatal(err)
	}
	if childEvent2.Frame.Review == f.mainEvent.Frame.Review || childEvent2.Frame.Review.Revision != 8 {
		t.Fatalf("expected updated Review rev 8, got %+v", childEvent2.Frame.Review)
	}
	if childEvent2.Frame.Family == f.mainEvent.Frame.Family || childEvent2.Frame.Family.Revision != 4 {
		t.Fatalf("expected updated Family rev 4, got %+v", childEvent2.Frame.Family)
	}
}

// Pre-upgrade daemons emit revisioned Reviews but omit Family.Revision and
// reject unknown view/attach params. Verify Attach omits Retain in that case.
func TestNativeAttachOmitsRetainWithoutFamilyRevision(t *testing.T) {
	review := &changes.ReviewView{Revision: 7}
	legacyFamily := &Family{Root: "main", Threads: []string{"main", "child"}}
	f := newAttachRetainFixture(t, review, legacyFamily,
		&activity.Publication{Entries: []activity.Entry{{Seq: 1, Kind: "assistant", Text: "main"}}, LastSeq: 1},
		&activity.Publication{Entries: []activity.Entry{{Seq: 1, Kind: "assistant", Text: "child"}}, LastSeq: 1},
	)
	if params, _, _ := f.mainConn.attachParams("child"); params.Retain != nil {
		t.Fatalf("expected Retain to be omitted when Family.Revision == 0, got %+v", params.Retain)
	}
	childConn, err := f.mainConn.Attach(t.Context(), "child")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = childConn.Close() })
	childEvent, err := childConn.Next()
	if err != nil {
		t.Fatal(err)
	}
	if childEvent.Frame.Review == f.mainEvent.Frame.Review || childEvent.Frame.Review.Revision != 7 {
		t.Fatalf("expected fresh Review rev 7 without Retain, got %+v", childEvent.Frame.Review)
	}
}

func TestFamilyNewEmptyChildKeepsReviewRevision(t *testing.T) {
	workspace := "/workspace"
	chunk := func(name, caller, diff string) changes.File {
		return changes.File{Path: name, Chunks: []changes.Chunk{{Workspace: workspace, Key: "chunk-" + name, Caller: caller, Review: diffmodel.ReviewFile{AfterPath: workspace + "/" + name, Diff: diff}}}}
	}
	h := NewHub(t.Context(), nil)
	t.Cleanup(h.Close)

	root, child := NewLive(), NewLive()
	_ = root.SetSummary(Summary{Thread: "main", Workspace: workspace, Ready: true})
	root.frame.Store(&Frame{Session: Session{Thread: "main", Workspace: workspace}, FamilyRoot: "main", Review: &changes.ReviewView{Workspace: workspace, Revision: 1, Publication: changes.Publication{Files: []changes.File{chunk("a.go", "/root", "+a\n")}}}})
	h.mu.Lock()
	h.sessions["main"] = &listedSession{entry: listed{Thread: "main", Workspace: workspace}, server: root}
	h.mu.Unlock()

	_, before := h.Family("main")
	_ = child.SetSummary(Summary{Thread: "child", Workspace: workspace, Ready: true})
	child.frame.Store(&Frame{Session: Session{Thread: "child", Workspace: workspace}, FamilyRoot: "main", Review: &changes.ReviewView{Workspace: workspace, Revision: 1}})
	h.mu.Lock()
	h.sessions["child"] = &listedSession{entry: listed{Thread: "child", Workspace: workspace, ParentThread: "main"}, server: child}
	h.mu.Unlock()

	if _, afterEmpty := h.Family("child"); afterEmpty != before || afterEmpty.Revision != before.Revision {
		t.Fatalf("empty child invalidated family review: before rev=%d, after rev=%d", before.Revision, afterEmpty.Revision)
	}

	child.frame.Store(&Frame{Session: Session{Thread: "child", Workspace: workspace}, FamilyRoot: "main", Review: &changes.ReviewView{Workspace: workspace, Revision: 2, Publication: changes.Publication{Files: []changes.File{chunk("b.go", "/root/worker", "+b\n")}}}})
	if _, afterEdit := h.Family("child"); afterEdit.Revision == before.Revision || len(afterEdit.Files) != 2 {
		t.Fatalf("child edit did not recompose family review: rev=%d files=%d", afterEdit.Revision, len(afterEdit.Files))
	}
}

func BenchmarkNativeAttachLargeFamily(b *testing.B) {
	files := make([]changes.File, 40)
	unfiltered := make(map[string]changes.File, 40)
	chunkDiff := strings.Repeat("+saved diff line with realistic code content here\n", 10000)
	for i := range files {
		path := fmt.Sprintf("pkg/file_%02d.go", i)
		file := changes.File{Path: path, Chunks: []changes.Chunk{{Key: fmt.Sprintf("chunk-%02d", i), Caller: "/root/worker-01", Review: diffmodel.ReviewFile{Diff: chunkDiff}}}}
		files[i], unfiltered[path] = file, file
	}
	review := &changes.ReviewView{Revision: 1, Publication: changes.Publication{Files: files, Unfiltered: unfiltered}}
	family := &Family{Root: "main", Revision: 1, Threads: []string{"main", "child"}, Agents: &activity.Publication{MainSummary: []activity.Entry{{Seq: 1, Kind: "agent", Text: strings.Repeat("summary ", 8192)}}}}
	f := newAttachRetainFixture(b, review, family, benchmarkNativeHistoryFrame(200).Main, benchmarkNativeHistoryFrame(200).Main)

	b.ReportAllocs()
	b.Run("WarmSameFamily", func(b *testing.B) {
		for b.Loop() {
			childConn, err := f.mainConn.Attach(b.Context(), "child")
			if err != nil {
				b.Fatal(err)
			}
			got, err := childConn.Next()
			if err != nil {
				b.Fatal(err)
			}
			if got.Frame.Review != f.mainEvent.Frame.Review || got.Frame.Family != f.mainEvent.Frame.Family {
				b.Fatal("expected Review and Family retention")
			}
			_ = childConn.Close()
		}
	})
	b.Run("ColdUpdatedFamilyReviewRetained", func(b *testing.B) {
		nextFamily := new(*family)
		nextFamily.Revision = 2
		f.childFamily = nextFamily
		for b.Loop() {
			childConn, err := f.mainConn.Attach(b.Context(), "child")
			if err != nil {
				b.Fatal(err)
			}
			got, err := childConn.Next()
			if err != nil {
				b.Fatal(err)
			}
			if got.Frame.Review != f.mainEvent.Frame.Review || got.Frame.Family == f.mainEvent.Frame.Family {
				b.Fatal("expected Review retained and updated Family delivered")
			}
			_ = childConn.Close()
		}
	})
}

