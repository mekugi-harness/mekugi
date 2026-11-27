package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"path/filepath"
	"slices"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

func TestDirectNativeInterruptRestoresUncommittedInput(t *testing.T) {
	for _, reject := range []bool{false, true} {
		name := "interrupt"
		if reject {
			name = "interrupt refused"
		}
		t.Run(name, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
			defer cancel()
			socket := filepath.Join(shortTempDir(t), "native.sock")
			srv := startMockNativeAppServer(t, socket)
			conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
			if err != nil {
				t.Fatal(err)
			}
			defer conn.Close()
			notify := func(method string, params map[string]any) {
				srv.send(t, map[string]any{"jsonrpc": "2.0", "method": method, "params": params})
			}
			commit := func(threadID, turnID, clientID, text string) {
				notify("item/completed", map[string]any{
					"threadId": threadID, "turnId": turnID,
					"item": map[string]any{
						"id": "user-" + clientID + "-" + threadID + "-" + turnID, "type": "userMessage", "clientId": clientID,
						"content": []map[string]any{{"type": "text", "text": text}},
					},
				})
			}
			notify("turn/started", map[string]any{"threadId": "thr-main", "turn": map[string]string{"id": "turn-active"}})
			awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Interrupt.Turn == "turn-active" })

			sendSteer := func(value string) {
				t.Helper()
				_, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Composer: &sessionapi.NativeComposerInput{
					Intent: sessionpresentation.ComposerIntent{Thread: "thr-main", Operation: sessionpresentation.SubmitComposer},
					Draft:  composer.Draft{Text: value},
				}})
				if err != nil {
					t.Fatal(err)
				}
			}
			// Deliver a commitment before admitting the RPC acknowledgement to
			// prove it cannot resurrect the already committed local draft.
			firstClient := make(chan string, 1)
			releaseSteer := make(chan struct{})
			srv.mu.Lock()
			srv.overrides["turn/steer"] = func(raw jsontext.Value) any {
				var p struct {
					ClientID string `json:"clientUserMessageId"`
				}
				_ = json.Unmarshal(raw, &p)
				firstClient <- p.ClientID
				select {
				case <-releaseSteer:
				case <-ctx.Done():
				}
				return map[string]string{"turnId": "turn-active"}
			}
			srv.mu.Unlock()
			steerDone := make(chan error, 1)
			go func() {
				_, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Composer: &sessionapi.NativeComposerInput{
					Intent: sessionpresentation.ComposerIntent{Thread: "thr-main", Operation: sessionpresentation.SubmitComposer},
					Draft:  composer.Draft{Text: "already committed"},
				}})
				steerDone <- err
			}()
			var clientID string
			select {
			case clientID = <-firstClient:
			case <-ctx.Done():
				t.Fatal("no steer request")
			}
			if clientID == "" {
				t.Fatal("turn/steer did not carry a client message ID")
			}
			commit("thr-main", "turn-active", clientID, "already committed")
			awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
				return ev.Frame.Main != nil && len(ev.Frame.Main.Entries) > 0
			})
			close(releaseSteer)
			if err := <-steerDone; err != nil {
				t.Fatal(err)
			}
			srv.mu.Lock()
			delete(srv.overrides, "turn/steer")
			srv.mu.Unlock()
			sendSteer("not yet committed")
			awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
				return slices.Equal(ev.Frame.View.Pending.Waiting, []presentation.PendingText{{Text: "not yet committed"}})
			})

			queued := map[string]any{"id": "queued", "kind": "message", "input": []map[string]any{{"type": "text", "text": "queued draft"}}}
			shell := map[string]any{"id": "shell", "kind": "shellCommand", "input": []map[string]any{{"type": "text", "text": "!pwd"}}}
			askedToInterrupt := make(chan struct{})
			srv.mu.Lock()
			srv.queueData = []map[string]any{queued, shell}
			srv.overrides["thread/queue/take"] = func(jsontext.Value) any { return map[string]any{"data": []any{queued}} }
			srv.overrides["turn/interrupt"] = func(jsontext.Value) any {
				close(askedToInterrupt)
				if reject {
					return mockRPCError("interrupt rejected")
				}
				return map[string]any{}
			}
			srv.mu.Unlock()
			interrupted := make(chan error, 1)
			go func() {
				_, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Interrupt: &sessionpresentation.InterruptIntent{
					Target: sessioncontrol.InterruptTarget{Thread: "thr-main", Turn: "turn-active"},
					Mode:   sessioncontrol.RestoreWaiting,
				}})
				interrupted <- err
			}()
			select {
			case <-askedToInterrupt:
			case <-ctx.Done():
				t.Fatal("no interrupt request")
			}
			if !reject {
				notify("turn/completed", map[string]any{"threadId": "thr-main", "turn": map[string]string{"id": "turn-active", "status": "interrupted"}})
			}
			if err := <-interrupted; (err != nil) != reject {
				t.Fatalf("interrupt result: %v", err)
			}
			ev := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return len(ev.Frame.Effects) != 0 })
			want := "not yet committed\nqueued draft"
			if reject {
				want = "queued draft"
			}
			if effects := ev.Frame.Effects; len(effects) != 1 || effects[0].Thread != "thr-main" ||
				effects[0].Restore == nil || effects[0].Restore.Text != want {
				t.Fatalf("restore effects: %+v, want one for %q", effects, want)
			}
			if got := len(ev.Frame.View.Pending.Waiting); (got != 0) != reject {
				t.Fatalf("remaining pending steers: %d", got)
			}
			srv.mu.Lock()
			defer srv.mu.Unlock()
			take, interrupt := slices.Index(srv.methods, "thread/queue/take"), slices.Index(srv.methods, "turn/interrupt")
			var request struct {
				IDs []string `json:"queuedSubmissionIds"`
			}
			if take < 0 || take >= interrupt || json.Unmarshal(srv.params["thread/queue/take"][0], &request) != nil ||
				!slices.Equal(request.IDs, []string{"queued"}) {
				t.Fatalf("queue take before interrupt: methods %v; params %s", srv.methods, srv.params["thread/queue/take"])
			}
			if len(srv.params["turn/steer"]) != 2 || len(srv.params["turn/start"]) != 0 {
				t.Fatal("interrupt automatically resent user input")
			}
		})
	}
}

func TestDirectNativeInterruptRestoresQueueSteerOnlyIfRemoved(t *testing.T) {
	for _, deleted := range []bool{true, false} {
		name := "locally removed"
		if !deleted {
			name = "already absent"
		}
		t.Run(name, func(t *testing.T) {
			f := newNativeParitySnapshot(t)
			f.server.mu.Lock()
			f.server.queueData = []map[string]any{{
				"id": "queued", "kind": "message", "clientUserMessageId": "queued-message",
				"input": []map[string]any{{"type": "text", "text": "queued draft"}},
			}}
			f.server.overrides["thread/queue/delete"] = func(jsontext.Value) any {
				f.server.mu.Lock()
				f.server.queueData = nil
				f.server.mu.Unlock()
				return map[string]bool{"deleted": deleted}
			}
			f.server.overrides["turn/interrupt"] = func(jsontext.Value) any {
				f.notify("turn/completed", `{"threadId":"thr-main","turn":{"id":"turn-active","status":"interrupted"}}`)
				return map[string]any{}
			}
			f.server.mu.Unlock()
			f.notify("turn/started", `{"threadId":"thr-main","turn":{"id":"turn-active","status":"inProgress"}}`)
			f.notify("thread/queue/changed", `{"threadId":"thr-main"}`)
			f.await(func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Pending.CanSteer })
			f.send(sessionapi.NativeOperation{Composer: &sessionapi.NativeComposerInput{
				Intent: sessionpresentation.ComposerIntent{Thread: "thr-main", Operation: sessionpresentation.SteerWaiting},
			}})
			result, err := f.conn.Send(f.ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
				Interrupt: &sessionpresentation.InterruptIntent{
					Target: sessioncontrol.InterruptTarget{Thread: "thr-main", Turn: "turn-active"},
					Mode:   sessioncontrol.RestoreWaiting,
				},
			})
			if err != nil {
				t.Fatal(err)
			}
			frame := f.await(func(ev sessionapi.NativeEvent) bool {
				return ev.Frame.View.Version.Revision >= result.Version.Revision
			}).Frame
			if deleted {
				if len(frame.Effects) != 1 || frame.Effects[0].Restore == nil || frame.Effects[0].Restore.Text != "queued draft" {
					t.Fatalf("did not restore the locally removed queue steer: %+v", frame.Effects)
				}
			} else if len(frame.Effects) != 0 {
				t.Fatalf("restored a queue steer removed elsewhere: %+v", frame.Effects)
			}
		})
	}
}
