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

func TestNativeAttachRetainsFamilyAndReviewAcrossSameConnection(t *testing.T) {
	workspace := t.TempDir()
	review := &changes.ReviewView{
		Workspace: workspace,
		Revision:  7,
		Publication: changes.Publication{
			Files: []changes.File{{
				Path: "large.go",
				Chunks: []changes.Chunk{{
					Workspace: workspace,
					Key:       "chunk-1",
					Caller:    "/root",
					Review:    diffmodel.ReviewFile{Diff: strings.Repeat("+saved diff line\n", 4096)},
				}},
			}},
		},
	}
	family := &Family{
		Root:     "main",
		Revision: 3,
		Threads:  []string{"main", "child"},
		Agents: &activity.Publication{
			Roster: []activity.AgentRow{
				{Agent: activity.Agent{AgentFacts: activity.AgentFacts{Name: "/root", Thread: "main"}}},
				{Agent: activity.Agent{AgentFacts: activity.AgentFacts{Name: "/root/worker", Thread: "child"}}, Depth: 1},
			},
			MainSummary: []activity.Entry{{Seq: 1, Kind: "agent", Text: strings.Repeat("summary ", 2048)}},
		},
	}

	childReview := review
	childFamily := family
	mainServer := NewLive()
	childServer := NewLive()
	_ = mainServer.SetSummary(Summary{Thread: "main", Workspace: workspace, Ready: true})
	_ = childServer.SetSummary(Summary{Thread: "child", Workspace: workspace, Ready: true})

	hub := NewHub(t.Context(), nil)
	t.Cleanup(hub.Close)
	hub.mu.Lock()
	hub.sessions["main"] = &listedSession{entry: listed{Thread: "main", Workspace: workspace}, server: mainServer}
	hub.sessions["child"] = &listedSession{entry: listed{Thread: "child", Workspace: workspace, ParentThread: "main"}, server: childServer}
	hub.mu.Unlock()

	serve := func(s *Server, id, caller string, getReview func() *changes.ReviewView, getFamily func() *Family) {
		go func() {
			for {
				select {
				case <-t.Context().Done():
					return
				case req := <-s.Native().Requests():
					thread := sessionpresentation.Thread{ID: id, Caller: caller}
					frame := NativeFrame{
						View: sessionpresentation.View{
							Version: sessionpresentation.Version{Attachment: 1, Revision: 1},
							Main:    thread, Displayed: thread, Input: thread,
							Info: sessionpresentation.MainInfo{Workspace: workspace, Caller: caller},
						},
						Main:   &activity.Publication{Entries: []activity.Entry{{Seq: 1, Kind: "assistant", Text: id + " transcript"}}, LastSeq: 1},
						Review: getReview(),
						Family: getFamily(),
					}
					if err := s.Native().Publish(req.Attachment, frame); err != nil {
						return
					}
					req.Reply <- ""
				}
			}
		}()
	}
	serve(mainServer, "main", "/root", func() *changes.ReviewView { return review }, func() *Family { return family })
	serve(childServer, "child", "/root/worker", func() *changes.ReviewView { return childReview }, func() *Family { return childFamily })

	httpServer := httptest.NewServer(Versioned(hub.Handler()))
	t.Cleanup(httpServer.Close)

	mainConn, err := OpenNative(t.Context(), httpServer.Client(), httpServer.URL, "main")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = mainConn.Close() })
	mainEvent, err := mainConn.Next()
	if err != nil {
		t.Fatal(err)
	}
	if mainEvent.Frame.Review == nil || mainEvent.Frame.Family == nil {
		t.Fatal("missing initial main Review or Family")
	}

	// Same Review and Family: child's initial snapshot must reuse both pointers.
	childConn, err := mainConn.Attach(t.Context(), "child")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = childConn.Close() })
	childEvent, err := childConn.Next()
	if err != nil {
		t.Fatal(err)
	}
	if childEvent.Frame.Review != mainEvent.Frame.Review {
		t.Fatal("child initial snapshot did not retain identical Review across same-connection attach")
	}
	if childEvent.Frame.Family != mainEvent.Frame.Family {
		t.Fatal("child initial snapshot did not retain identical Family across same-connection attach")
	}
	if len(childEvent.Frame.Main.Entries) != 1 || childEvent.Frame.Main.Entries[0].Text != "child transcript" {
		t.Fatalf("unexpected child main transcript: %+v", childEvent.Frame.Main)
	}

	// Updated Review and Family: child's initial snapshot must deliver the new owners.
	updatedReview := new(*review)
	updatedReview.Revision = 8
	updatedFamily := new(*family)
	updatedFamily.Revision = 4
	childReview, childFamily = updatedReview, updatedFamily

	childConn2, err := mainConn.Attach(t.Context(), "child")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = childConn2.Close() })
	childEvent2, err := childConn2.Next()
	if err != nil {
		t.Fatal(err)
	}
	if childEvent2.Frame.Review == mainEvent.Frame.Review || childEvent2.Frame.Review.Revision != 8 {
		t.Fatalf("expected updated Review rev 8, got %+v", childEvent2.Frame.Review)
	}
	if childEvent2.Frame.Family == mainEvent.Frame.Family || childEvent2.Frame.Family.Revision != 4 {
		t.Fatalf("expected updated Family rev 4, got %+v", childEvent2.Frame.Family)
	}
}

// Pre-upgrade daemons emit revisioned Reviews but omit Family.Revision and
// reject unknown view/attach params. Verify Attach omits Retain in that case.
func TestNativeAttachOmitsRetainWithoutFamilyRevision(t *testing.T) {
	workspace := t.TempDir()
	review := &changes.ReviewView{
		Workspace: workspace,
		Revision:  7,
		Publication: changes.Publication{
			Files: []changes.File{{
				Path: "a.go",
				Chunks: []changes.Chunk{{
					Workspace: workspace,
					Key:       "chunk-a",
					Caller:    "/root",
					Review:    diffmodel.ReviewFile{AfterPath: workspace + "/a.go", Diff: "+a\n"},
				}},
			}},
		},
	}
	// Revision == 0 models a pre-upgrade server that does not support retain.
	legacyFamily := &Family{Root: "main", Threads: []string{"main", "child"}}

	mainServer := NewLive()
	childServer := NewLive()
	_ = mainServer.SetSummary(Summary{Thread: "main", Workspace: workspace, Ready: true})
	_ = childServer.SetSummary(Summary{Thread: "child", Workspace: workspace, Ready: true})

	hub := NewHub(t.Context(), nil)
	t.Cleanup(hub.Close)
	hub.mu.Lock()
	hub.sessions["main"] = &listedSession{entry: listed{Thread: "main", Workspace: workspace}, server: mainServer}
	hub.sessions["child"] = &listedSession{entry: listed{Thread: "child", Workspace: workspace, ParentThread: "main"}, server: childServer}
	hub.mu.Unlock()

	serve := func(s *Server, id, caller string) {
		go func() {
			for {
				select {
				case <-t.Context().Done():
					return
				case req := <-s.Native().Requests():
					thread := sessionpresentation.Thread{ID: id, Caller: caller}
					frame := NativeFrame{
						View: sessionpresentation.View{
							Version: sessionpresentation.Version{Attachment: 1, Revision: 1},
							Main:    thread, Displayed: thread, Input: thread,
							Info: sessionpresentation.MainInfo{Workspace: workspace, Caller: caller},
						},
						Main:   &activity.Publication{Entries: []activity.Entry{{Seq: 1, Kind: "assistant", Text: id}}, LastSeq: 1},
						Review: review,
						Family: legacyFamily,
					}
					if err := s.Native().Publish(req.Attachment, frame); err != nil {
						return
					}
					req.Reply <- ""
				}
			}
		}()
	}
	serve(mainServer, "main", "/root")
	serve(childServer, "child", "/root/worker")

	httpServer := httptest.NewServer(Versioned(hub.Handler()))
	t.Cleanup(httpServer.Close)

	mainConn, err := OpenNative(t.Context(), httpServer.Client(), httpServer.URL, "main")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = mainConn.Close() })
	mainEvent, err := mainConn.Next()
	if err != nil {
		t.Fatal(err)
	}
	if mainEvent.Frame.Review == nil || mainEvent.Frame.Review.Revision != 7 || mainEvent.Frame.Family.Revision != 0 {
		t.Fatalf("unexpected initial legacy frame: review=%+v family=%+v", mainEvent.Frame.Review, mainEvent.Frame.Family)
	}

	childConn, err := mainConn.Attach(t.Context(), "child")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = childConn.Close() })
	childEvent, err := childConn.Next()
	if err != nil {
		t.Fatal(err)
	}
	// Without Family.Revision, Retain was omitted so the server sent a full snapshot.
	if childEvent.Frame.Review == mainEvent.Frame.Review {
		t.Fatal("expected full Review snapshot when Family.Revision is zero (legacy server)")
	}
	if childEvent.Frame.Review == nil || childEvent.Frame.Review.Revision != 7 {
		t.Fatalf("unexpected child review: %+v", childEvent.Frame.Review)
	}
}

func TestFamilyNewEmptyChildKeepsReviewRevision(t *testing.T) {
	workspace := "/workspace"
	review := &changes.ReviewView{
		Workspace: workspace,
		Revision:  1,
		Publication: changes.Publication{
			Files: []changes.File{{
				Path: "a.go",
				Chunks: []changes.Chunk{{
					Workspace: workspace,
					Key:       "chunk-a",
					Caller:    "/root",
					Review:    diffmodel.ReviewFile{AfterPath: workspace + "/a.go", Diff: "+a\n"},
				}},
			}},
		},
	}
	h := NewHub(t.Context(), nil)
	t.Cleanup(h.Close)

	root := NewLive()
	_ = root.SetSummary(Summary{Thread: "main", Workspace: workspace, Ready: true})
	root.frame.Store(&Frame{Session: Session{Thread: "main", Workspace: workspace}, FamilyRoot: "main", Review: review})
	h.mu.Lock()
	h.sessions["main"] = &listedSession{entry: listed{Thread: "main", Workspace: workspace}, server: root}
	h.mu.Unlock()

	_, before := h.Family("main")
	if before == nil || before.Revision == 0 {
		t.Fatalf("expected non-empty initial family review, got %+v", before)
	}

	child := NewLive()
	_ = child.SetSummary(Summary{Thread: "child", Workspace: workspace, Ready: true})
	child.frame.Store(&Frame{Session: Session{Thread: "child", Workspace: workspace}, FamilyRoot: "main", Review: &changes.ReviewView{Workspace: workspace, Revision: 1}})
	h.mu.Lock()
	h.sessions["child"] = &listedSession{entry: listed{Thread: "child", Workspace: workspace, ParentThread: "main"}, server: child}
	h.mu.Unlock()

	_, afterEmpty := h.Family("child")
	if afterEmpty != before || afterEmpty.Revision != before.Revision {
		t.Fatalf("empty child invalidated family review: before rev=%d, after rev=%d", before.Revision, afterEmpty.Revision)
	}

	// Once the child actually contributes a chunk, the family review must recompose and bump revision.
	child.frame.Store(&Frame{
		Session:    Session{Thread: "child", Workspace: workspace},
		FamilyRoot: "main",
		Review: &changes.ReviewView{
			Workspace: workspace,
			Revision:  2,
			Publication: changes.Publication{
				Files: []changes.File{{
					Path: "b.go",
					Chunks: []changes.Chunk{{
						Workspace: workspace,
						Key:       "chunk-b",
						Caller:    "/root/worker",
						Review:    diffmodel.ReviewFile{AfterPath: workspace + "/b.go", Diff: "+b\n"},
					}},
				}},
			},
		},
	})
	_, afterEdit := h.Family("child")
	if afterEdit.Revision == before.Revision || len(afterEdit.Files) != 2 {
		t.Fatalf("child edit did not recompose family review: rev=%d files=%d", afterEdit.Revision, len(afterEdit.Files))
	}
}

func BenchmarkNativeAttachLargeFamily(b *testing.B) {
	workspace := b.TempDir()
	files := make([]changes.File, 40)
	unfiltered := make(map[string]changes.File, 40)
	chunkDiff := strings.Repeat("+saved diff line with realistic code content here\n", 10000)
	for i := range files {
		path := fmt.Sprintf("pkg/file_%02d.go", i)
		f := changes.File{
			Path: path,
			Chunks: []changes.Chunk{{
				Workspace: workspace,
				Key:       fmt.Sprintf("chunk-%02d", i),
				Caller:    "/root/worker-01",
				Review:    diffmodel.ReviewFile{Diff: chunkDiff},
			}},
		}
		files[i] = f
		unfiltered[path] = f
	}
	review := &changes.ReviewView{
		Workspace:   workspace,
		Revision:    1,
		Publication: changes.Publication{Files: files, Unfiltered: unfiltered},
	}
	family := &Family{
		Root:     "main",
		Revision: 1,
		Threads:  []string{"main", "child"},
		Agents: &activity.Publication{
			MainSummary: []activity.Entry{{Seq: 1, Kind: "agent", Text: strings.Repeat("summary ", 8192)}},
		},
	}
	mainHist := benchmarkNativeHistoryFrame(200).Main
	childHist := benchmarkNativeHistoryFrame(200).Main

	mainServer := NewLive()
	childServer := NewLive()
	_ = mainServer.SetSummary(Summary{Thread: "main", Workspace: workspace, Ready: true})
	_ = childServer.SetSummary(Summary{Thread: "child", Workspace: workspace, Ready: true})

	hub := NewHub(b.Context(), nil)
	b.Cleanup(hub.Close)
	hub.mu.Lock()
	hub.sessions["main"] = &listedSession{entry: listed{Thread: "main", Workspace: workspace}, server: mainServer}
	hub.sessions["child"] = &listedSession{entry: listed{Thread: "child", Workspace: workspace, ParentThread: "main"}, server: childServer}
	hub.mu.Unlock()

	ctx, cancel := context.WithCancel(b.Context())
	b.Cleanup(cancel)
	serve := func(s *Server, id, caller string, hist *activity.Publication) {
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
						Review: review,
						Family: family,
					}
					if err := s.Native().Publish(req.Attachment, frame); err != nil {
						return
					}
					req.Reply <- ""
				}
			}
		}()
	}
	serve(mainServer, "main", "/root", mainHist)
	serve(childServer, "child", "/root/worker", childHist)

	httpServer := httptest.NewServer(Versioned(hub.Handler()))
	b.Cleanup(httpServer.Close)
	mainConn, err := OpenNative(b.Context(), httpServer.Client(), httpServer.URL, "main")
	if err != nil {
		b.Fatal(err)
	}
	b.Cleanup(func() { _ = mainConn.Close() })
	if _, err := mainConn.Next(); err != nil {
		b.Fatal(err)
	}

	b.ReportAllocs()
	b.ResetTimer()
	b.Run("WarmSameFamily", func(b *testing.B) {
		for b.Loop() {
			childConn, err := mainConn.Attach(b.Context(), "child")
			if err != nil {
				b.Fatal(err)
			}
			if _, err := childConn.Next(); err != nil {
				b.Fatal(err)
			}
			_ = childConn.Close()
		}
	})
	b.Run("ColdUpdatedFamilyReviewRetained", func(b *testing.B) {
		nextFamily := new(*family)
		nextFamily.Revision = 2
		family = nextFamily
		for b.Loop() {
			childConn, err := mainConn.Attach(b.Context(), "child")
			if err != nil {
				b.Fatal(err)
			}
			if _, err := childConn.Next(); err != nil {
				b.Fatal(err)
			}
			_ = childConn.Close()
		}
	})
}

