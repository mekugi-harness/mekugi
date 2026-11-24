package launch

import (
	json "encoding/json/v2"
	"fmt"
	"math"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
)

func TestAPILiveRetainedDelegateUsesSelectedPresentationIdentity(t *testing.T) {
	u, _ := newAppServerTestUI()
	workspace := t.TempDir()
	u.Startup.Thread = "child"
	appServerTestRequest(t, u, "1", "thread/resume")
	appServerTestMessage(t, u, fmt.Sprintf(`{"id":1,"result":{"thread":{"id":"child","parentThreadId":"parent","source":{"subAgent":{"thread_spawn":{"agent_path":"/root/worker"}}},"cwd":%q,"turns":[]}}}`, workspace))
	if u.attached.threadInput != nil {
		t.Fatal("retained startup fixture unexpectedly had native input metadata")
	}
	h := remoteOwnerView(t, u)
	publisher := apiLive{server: h.api, started: time.Now()}
	publisher.publish(&u.attached, &u.Client, nil, u.reviewSource, false, false, time.Now())
	events := serverRPCSubscribe(t, h.rpc, "child")
	event, err := events.Next(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	var state struct {
		Session sessionapi.Session `json:"session"`
	}
	if err := json.Unmarshal(event.Data, &state); err != nil {
		t.Fatal(err)
	}
	if state.Session.Thread != "child" || state.Session.Caller != "/root/worker" {
		t.Fatalf("API publication lost retained delegate identity: %+v", state.Session)
	}
}

func TestRetainedThreadResumeUsesNativeIdentityWithoutCachedAgent(t *testing.T) {
	for _, test := range []struct {
		name, parent, source, caller string
	}{
		{"delegate", "parent", `{"subAgent":{"thread_spawn":{"agent_path":"/root/worker"}}}`, "/root/worker"},
		{"delegate without path", "parent", `null`, "/root/retained"},
		{"root", "", `null`, "/root"},
	} {
		t.Run(test.name, func(t *testing.T) {
			u, _ := newAppServerTestUI()
			u.Startup.Thread = "retained-thread"
			appServerTestRequest(t, u, "1", "thread/resume")
			appServerTestMessage(t, u, fmt.Sprintf(`{"id":1,"result":{"thread":{"id":"retained-thread","parentThreadId":%q,"source":%s,"cwd":"/retained","turns":[]}}}`, test.parent, test.source))
			if u.nativeAgent != nil {
				t.Fatal("resume fixture unexpectedly had a cached native agent")
			}
			if got := u.attached.session.KnownCaller("retained-thread"); got != test.caller {
				t.Fatalf("retained thread caller = %q, want %q", got, test.caller)
			}
			view := u.attached.presentation.View()
			if view.Main.Caller != test.caller || view.Displayed != view.Main {
				t.Fatalf("retained thread selected another identity: %+v", view.Main)
			}
			publication := u.attached.childSource.Publication(u.attached.presentation, u.attached.source)
			if len(publication.Roster) != 1 || publication.Roster[0].Agent.Name != test.caller || publication.Roster[0].Agent.Thread != "retained-thread" {
				t.Fatalf("retained roster lost its native identity: %+v", publication.Roster)
			}
			if test.name == "delegate without path" {
				canonical := binding.ThreadInfo{ID: "retained-thread", ParentThreadID: "parent", Source: []byte(`{"subAgent":{"thread_spawn":{"agent_path":"/root/later"}}}`)}
				c := &nativeConnection{families: map[string]*nativeFamily{"parent": {response: "1", threads: []binding.ThreadInfo{canonical}}}}
				c.projectAgent(&u.nativeSession)
				if view := u.attached.presentation.View(); view.Main.Caller != "/root/later" || view.Displayed != view.Main {
					t.Fatalf("later native metadata did not rename selected thread: %+v", view.Main)
				}
				publication = u.attached.childSource.Publication(u.attached.presentation, u.attached.source)
				if publication.Roster[0].Agent.Name != "/root/later" {
					t.Fatalf("later native metadata did not rename roster: %+v", publication.Roster)
				}
			}
		})
	}
}

func TestNativeViewUsesSelectedDelegateCanonicalIdentity(t *testing.T) {
	u, _ := newAppServerTestUI()
	workspace := t.TempDir()
	u.Startup.Thread = "child"
	appServerTestRequest(t, u, "1", "thread/resume")
	appServerTestMessage(t, u, fmt.Sprintf(`{"id":1,"result":{"thread":{"id":"child","parentThreadId":"parent","source":{"subAgent":{"thread_spawn":{"agent_path":"/root/worker"}}},"cwd":%q,"turns":[]}}}`, workspace))
	u.attached.session.Activity().Agent("child").ContextTokens = 1000
	u.attached.session.RegisterThread(observation.Thread{ID: "parent", Path: "/root"})
	u.attached.session.Activity().Agent("parent").ContextTokens = 9000
	h := remoteOwnerView(t, u)
	if view := h.event.Frame.View; view.Main.Caller != "/root/worker" || view.Info.Caller != "/root/worker" || view.Displayed != view.Main || view.Caption.ContextTokens != 1000 {
		t.Fatalf("native child attachment borrowed parent identity or caption: %+v", view)
	}
	u.attached.presentation.RenameCaller("/root/worker", "/root/renamed")
	u.attached.session.Activity().Agent("child").Name = "/root/renamed"
	h.views.publish(&u.attached, &u.Client, u.settings, "", "", false, nil, nil, u.reviewSource, sessionapi.NativeSessionFacts{}, time.Now().Add(time.Second))
	if view := h.view.attached.presentation.View(); view.Main.Caller != "/root/renamed" || view.Displayed != view.Main || view.Caption.ContextTokens != 1000 {
		t.Fatalf("native child view did not follow canonical rename: %+v", view)
	}
	h.view.initial = false
	h.views.replacing = h.event.Attachment
	u.attached = newAttachedPresentation(t.Context(), u.presentationState, u.conversationState, u.observations, u.attached.hooks)
	u.attached.session.RegisterThread(observation.Thread{ID: "child", Path: "/root/renamed"})
	u.startSession("child", workspace)
	u.attached.presentation.Attach(sessionpresentation.Thread{ID: "child", Caller: "/root/renamed"}, false)
	h.views.publish(&u.attached, &u.Client, u.settings, "", "", false, nil, nil, u.reviewSource, sessionapi.NativeSessionFacts{}, time.Now().Add(2*time.Second))
	if view := h.view.attached.presentation.View(); view.Main.Caller != "/root/renamed" || view.Displayed != view.Main {
		t.Fatalf("replacement child attachment changed canonical identity: %+v", view.Main)
	}
}

func TestConnectedChildCanonicalIdentityAndMetrics(t *testing.T) {
	u, _ := newAppServerTestUI()
	parent := binding.ThreadInfo{ID: "parent", Source: []byte(`{"subAgent":{"thread_spawn":{"agent_path":"/root"}}}`), ContextTokens: new(uint64(9000))}
	child := binding.ThreadInfo{ID: "child", ParentThreadID: "parent", AgentRole: "worker", Source: []byte(`{"subAgent":{"thread_spawn":{"agent_path":"/root/worker"}}}`), ContextTokens: new(uint64(1000))}
	u.nativeAgent = &child
	u.Startup.Thread = child.ID
	appServerTestRequest(t, u, "1", "thread/resume")
	appServerTestMessage(t, u, `{"id":1,"result":{"thread":{"id":"child","cwd":"/child","turns":[]}}}`)
	if got := u.attached.session.KnownCaller(child.ID); got != "/root/worker" {
		t.Fatalf("child startup identity = %q", got)
	}
	if view := u.attached.presentation.View(); view.Main.Caller != "/root/worker" || view.Displayed != view.Main {
		t.Fatalf("child startup selected another identity: %+v", view.Main)
	}
	u.usage = accounting.New()
	notifyTestProviderUsage(t, u, "child", "child-response", "gpt-6.2-sol", true, 1234, 56)
	notifyTestProviderUsage(t, u, "parent", "parent-response", "gpt-6.2-sol", true, 319800, 1900)
	c := &nativeConnection{root: &u.nativeSession, families: map[string]*nativeFamily{"parent": {observed: true}}}
	result, err := json.Marshal(struct {
		Data []binding.ThreadInfo `json:"data"`
	}{Data: []binding.ThreadInfo{parent, child}})
	if err != nil {
		t.Fatal(err)
	}
	if err := c.treeResponse("parent", appserver.Message{ID: []byte("1"), Result: result}); err != nil {
		t.Fatal(err)
	}
	c.projectAgent(&u.nativeSession)
	applyObservedActivity(&u.attached, &u.Client, u.usage, nil, nil, nil, u.clock, nil, nil)
	publication := u.attached.childSource.Publication(u.attached.presentation, u.attached.source)
	if len(publication.Roster) != 2 {
		t.Fatalf("native family roster = %+v", publication.Roster)
	}
	for _, row := range publication.Roster {
		agent := row.Agent
		switch agent.Thread {
		case "parent":
			if agent.Name != "/root" || agent.ContextTokens != 9000 || agent.InputTokens != 319800 || agent.OutputTokens != 1900 || agent.Roundtrips != 1 || math.Abs(agent.Cost-1.3077) > 1e-10 {
				t.Fatalf("parent metrics = %+v", agent)
			}
		case "child":
			if agent.Name != "/root/worker" || agent.ContextTokens != 1000 || agent.InputTokens != 1234 || agent.OutputTokens != 56 || agent.Roundtrips != 1 || math.Abs(agent.Cost-0.003028) > 1e-10 {
				t.Fatalf("child metrics = %+v", agent)
			}
		}
	}
	appServerTestMessage(t, u, `{"method":"thread/tokenUsage/updated","params":{"threadId":"child","tokenUsage":{"last":{"totalTokens":2000},"total":{"totalTokens":5000}}}}`)
	if agent := u.attached.session.Activity().Agent("child"); agent.Name != "/root/worker" || agent.ContextTokens != 2000 {
		t.Fatalf("live child metrics lost native identity: %+v", agent)
	}
	if agent := u.attached.session.Activity().Agent("parent"); agent.ContextTokens != 9000 {
		t.Fatalf("child context update mutated parent: %+v", agent)
	}
}
