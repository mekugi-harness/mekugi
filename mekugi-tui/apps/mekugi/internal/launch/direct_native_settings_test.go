package launch

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presets"
)

func TestDirectNativeResumeForwardsOnlyExplicitSettings(t *testing.T) {
	for _, test := range []struct {
		name string
		args []string
		want map[string]any
	}{
		{name: "none", want: map[string]any{}},
		{
			name: "model only", args: []string{"-m", "gpt-6-astra"},
			want: map[string]any{"model": "gpt-6-astra"},
		},
		{
			name: "effort and tier", args: []string{"--low", "-c", `service_tier="flex"`},
			want: map[string]any{"config": map[string]any{"model_reasoning_effort": "low"}, "serviceTier": "flex"},
		},
		{
			name: "provider only", args: []string{"--config=model_provider='custom'"},
			want: map[string]any{"modelProvider": "custom"},
		},
	} {
		t.Run(test.name, func(t *testing.T) {
			socket := filepath.Join(shortTempDir(t), "native.sock")
			server := startMockNativeAppServer(t, socket)
			server.overrides["thread/resume"] = func(jsontext.Value) any {
				return jsontext.Value(`{"thread":{"id":"thr-saved","cwd":"/saved"}}`)
			}
			conn, err := openDirectNativeConnection(t.Context(), socket, "/tmp/ws", nil, test.args, "thr-saved", "")
			if err != nil {
				t.Fatal(err)
			}
			defer conn.Close()
			test.want["threadId"] = "thr-saved"
			server.mu.Lock()
			defer server.mu.Unlock()
			var got map[string]any
			if calls := server.params["thread/resume"]; len(calls) != 1 ||
				json.Unmarshal(calls[0], &got) != nil || !reflect.DeepEqual(got, test.want) {
				t.Fatalf("thread/resume requests = %s, want %+v", calls, test.want)
			}
		})
	}
}

func TestDirectNativePermissionsKeepRejectedSettings(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "permissions.sock")
	server := startMockNativeAppServer(t, socket)
	server.overrides["thread/start"] = func(jsontext.Value) any {
		return jsontext.Value(`{"thread":{"id":"thr-main","cwd":"/stored"},"cwd":"/effective","activePermissionProfile":{"id":":workspace"}}`)
	}
	server.overrides["permissionProfile/list"] = func(jsontext.Value) any {
		return jsontext.Value(`{"data":[{"id":":workspace","allowed":true},{"id":"project","description":"Project access","allowed":true},{"id":"unavailable","allowed":false}],"nextCursor":null}`)
	}
	ctx := t.Context()
	conn, err := openDirectNativeConnection(ctx, socket, "/launch", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	send := func(op sessionapi.NativeOperation) error {
		_, err := conn.Send(ctx, sessionapi.NativeTarget{}, op)
		return err
	}
	do := func(op sessionapi.NativeOperation) {
		t.Helper()
		if err := send(op); err != nil {
			t.Fatal(err)
		}
	}
	open := func() *sessionpresentation.MenuView {
		t.Helper()
		do(sessionapi.NativeOperation{Command: new("/permissions")})
		return awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool {
			return event.Frame.View.Menu != nil && event.Frame.View.Menu.Setting == "/permissions"
		}).Frame.View.Menu
	}
	menu := open()
	if !menu.Choices[0].Current || !menu.Choices[2].Unavailable {
		t.Fatalf("permission choices: %+v", menu.Choices)
	}
	if err := send(sessionapi.NativeOperation{Menu: &sessionpresentation.MenuAction{Menu: menu.ID, Kind: sessionpresentation.MenuAccept, Choice: completion.ChoiceID{Name: "unavailable"}}}); err == nil {
		t.Fatal("unavailable profile accepted")
	}
	do(sessionapi.NativeOperation{Menu: &sessionpresentation.MenuAction{Menu: menu.ID, Kind: sessionpresentation.MenuDismiss}})
	menu = open()
	do(sessionapi.NativeOperation{Menu: &sessionpresentation.MenuAction{Menu: menu.ID, Kind: sessionpresentation.MenuAccept, Choice: completion.ChoiceID{Name: "project"}}})
	do(sessionapi.NativeOperation{Command: new("/permissions project --approval-policy=on-request --reviewer=user")})
	_ = open()
	do(sessionapi.NativeOperation{Command: new("/session")})
	server.mu.Lock()
	defer server.mu.Unlock()
	for _, raw := range server.params["permissionProfile/list"] {
		if string(raw) != `{"threadId":"thr-main"}` {
			t.Fatalf("permission discovery must scope the catalog to Main: %s", raw)
		}
	}
	updates := server.params["thread/settings/update"]
	if len(updates) != 2 || !strings.Contains(string(updates[0]), `"permissions":"project"`) ||
		!strings.Contains(string(updates[1]), `"approvalPolicy":"on-request"`) || !strings.Contains(string(updates[1]), `"approvalsReviewer":"user"`) {
		t.Fatalf("unexpected permission updates: %s", updates)
	}
}

func TestDirectNativeSessionFeaturesScopeToTheDisplayedThread(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "native.sock")
	server := startMockNativeAppServer(t, socket)
	server.mu.Lock()
	server.overrides["thread/subtree/list"] = func(params jsontext.Value) any {
		if strings.Contains(string(params), `"thr-worker"`) {
			return jsontext.Value(`{"data":[{"id":"thr-worker"},{"id":"thr-grandchild"}]}`)
		}
		return jsontext.Value(`{"data":[{"id":"thr-main"},{"id":"thr-worker"},{"id":"thr-grandchild"}]}`)
	}
	server.overrides["featureUse/read"] = func(params jsontext.Value) any {
		if string(params) != `{"threads":["thr-worker","thr-grandchild"]}` {
			return mockRPCError("not the selected subtree")
		}
		return jsontext.Value(`{"threads":{"thr-worker":{"spawns":2},"thr-grandchild":{"spawns":3}}}`)
	}
	server.mu.Unlock()
	ctx := t.Context()
	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	send := func(op sessionapi.NativeOperation) {
		t.Helper()
		if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, op); err != nil {
			t.Fatal(err)
		}
	}
	send(sessionapi.NativeOperation{Display: &sessionpresentation.DisplayIntent{
		Thread: sessionpresentation.Thread{ID: "thr-worker", Caller: "/root/worker"},
	}})
	send(sessionapi.NativeOperation{Command: new("/session")})
	overview := awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool {
		return event.Frame.View.Panel != nil && event.Frame.View.Panel.Session
	})
	var panel terminalsession.StatusPanel
	panel.Update(overview.Frame.View)
	action, _ := panel.Key("\t")
	if panel.Title != "Session · Features" || action.Kind != sessionpresentation.PanelFeatures {
		t.Fatalf("Tab did not request the Features view: %q, %+v", panel.Title, action)
	}
	send(sessionapi.NativeOperation{Panel: &action})
	features := awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool {
		return event.Frame.View.Panel != nil && len(event.Frame.View.Panel.FeatureFields) > 0
	})
	panel.Update(features.Frame.View)
	total := slices.IndexFunc(panel.Fields, func(field sessionpresentation.StatusField) bool {
		return field.Group == "Spawns" && field.Label == "Total"
	})
	if total < 0 || !slices.Equal(panel.Fields[total].Values, []string{"2", "3"}) {
		t.Fatalf("selected-thread Self and Descendants: %+v", panel.Fields)
	}
	panel.Key("\t")
	if panel.Title != "Session · Overview" {
		t.Fatalf("Tab must return to aggregate Overview; found %q", panel.Title)
	}
}

func TestDirectNativePresetUsesScopedRPCsAndConfirmsReplacement(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "presets.sock")
	server := startMockNativeAppServer(t, socket)
	stored := presets.Slot{Input: "old prompt", Label: "Old"}
	server.overrides["thread/presets/list"] = func(raw jsontext.Value) any {
		var request struct {
			ThreadID string `json:"threadId"`
		}
		if err := json.Unmarshal(raw, &request, json.RejectUnknownMembers(true)); err != nil || request.ThreadID != "thr-main" {
			return mockRPCError("expected to list only the Main presets")
		}
		server.mu.Lock()
		defer server.mu.Unlock()
		slots := map[string]presets.Slot{}
		if stored.Input != "" {
			slots["1"] = stored
		}
		return map[string]any{"presets": slots}
	}
	server.overrides["thread/presets/write"] = func(raw jsontext.Value) any {
		var request struct {
			ThreadID string        `json:"threadId"`
			Slot     int           `json:"slot"`
			Expected *presets.Slot `json:"expected"`
			Preset   *presets.Slot `json:"preset"`
		}
		if err := json.Unmarshal(raw, &request, json.RejectUnknownMembers(true)); err != nil || request.ThreadID != "thr-main" || request.Slot != 1 {
			return mockRPCError("expected a scoped write to slot 1")
		}
		server.mu.Lock()
		defer server.mu.Unlock()
		if request.Expected == nil || *request.Expected != stored {
			return mockRPCError("slot changed since it was shown")
		}
		stored = presets.Slot{}
		if request.Preset != nil {
			stored = *request.Preset
		}
		return map[string]any{"preset": request.Preset}
	}
	ctx := t.Context()
	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	send := func(op sessionapi.NativeOperation) sessionapi.NativeEvent {
		t.Helper()
		result, err := conn.Send(ctx, sessionapi.NativeTarget{}, op)
		if err != nil {
			t.Fatal(err)
		}
		return awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool {
			return event.Frame.View.Version.Revision == result.Version.Revision
		})
	}
	open := func() sessionapi.NativeEvent {
		return send(sessionapi.NativeOperation{Command: new("/presets")})
	}
	choose := func(event sessionapi.NativeEvent, key, query string) sessionapi.NativeEvent {
		t.Helper()
		menu := event.Frame.View.Menu
		index := slices.IndexFunc(menu.Choices, func(choice completion.Choice) bool { return choice.Name == key })
		if index < 0 {
			t.Fatalf("choice %q missing from %s: %+v", key, menu.Kind, menu.Choices)
		}
		return send(sessionapi.NativeOperation{Menu: &sessionpresentation.MenuAction{
			Menu: menu.ID, Kind: sessionpresentation.MenuAccept, Choice: menu.Choices[index].Identity(), Query: query,
		}})
	}
	event := open()
	if event.Frame.View.Menu.Choices[0].Display != "1  Old" || event.Frame.View.Menu.Choices[9].Name != "0" {
		t.Fatalf("preset slot order or configured label: %+v", event.Frame.View.Menu.Choices)
	}
	event = choose(event, "1", "")
	if last := event.Frame.Effects[len(event.Frame.Effects)-1]; last.Recall == nil || *last.Recall != "old prompt" {
		t.Fatalf("recall must fill composer without submission: %+v", last)
	}
	editor := &sessionapi.NativeEditorInput{
		Draft:  composer.Draft{Text: "prefix $Ol suffix", CursorBack: len(" suffix")},
		Target: conversation.EditorTarget{Revision: 1},
	}
	event = send(sessionapi.NativeOperation{Completion: editor})
	named := event.Frame.View.Menu
	if named == nil || len(named.Choices) != 1 || !named.Choices[0].Preset {
		t.Fatalf("named preset not offered in $ completion: %+v", named)
	}
	event = send(sessionapi.NativeOperation{Editor: editor, Menu: &sessionpresentation.MenuAction{
		Menu: named.ID, Kind: sessionpresentation.MenuAccept, Choice: named.Choices[0].Identity(),
	}})
	effects := event.Frame.Effects[len(event.Frame.Effects)-2:]
	if remove, insert := effects[0].Action, effects[1].Action; remove == nil || remove.Start != len("prefix ") ||
		remove.End != len("prefix $Ol") || insert == nil || insert.Text != "old prompt" {
		t.Fatalf("named preset did not replace just its completion token: %+v", effects)
	}
	event = choose(open(), "e", "")
	event = choose(event, "1", "")
	if menu := event.Frame.View.Menu; menu.Kind != "preset-edit" || menu.Query != "old prompt" {
		t.Fatalf("edit seed: %+v", menu)
	}
	event = choose(event, "continue", "replacement prompt")
	event = choose(event, "continue", "")
	var menu terminalsession.Menu
	menu.Update(event.Frame.View)
	if got := menu.Key("\r"); got.Action.Kind != sessionpresentation.MenuNoAction {
		t.Fatalf("Enter confirmed a replacement: %+v", got)
	}
	action := menu.Key("y").Action
	server.mu.Lock()
	stored = presets.Slot{Input: "changed elsewhere", Label: "Newer"}
	server.mu.Unlock()
	result, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Menu: &action})
	if err == nil {
		t.Fatal("stale replacement was not rejected by the native server")
	}
	event = awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool {
		return event.Frame.View.Version.Revision == result.Version.Revision
	})
	if menu := event.Frame.View.Menu; menu == nil || menu.Kind != "preset-confirm" || menu.Problem == "" {
		t.Fatalf("stale replacement must keep confirmation and show the failure: %+v", menu)
	}
	event = choose(open(), "e", "")
	event = choose(event, "1", "")
	if event.Frame.View.Menu.Query != "changed elsewhere" {
		t.Fatalf("reopen did not read the newer slot: %+v", event.Frame.View.Menu)
	}
	event = choose(event, "continue", "replacement prompt")
	event = choose(event, "continue", "")
	menu.Update(event.Frame.View)
	action = menu.Key("y").Action
	event = send(sessionapi.NativeOperation{Menu: &action})
	if event.Frame.View.Menu.Choices[0].Display != "1  replacement prompt" {
		t.Fatalf("untitled saved preset must display its first line: %+v", event.Frame.View.Menu)
	}
	event = choose(event, "d", "")
	event = choose(event, "1", "")
	event = choose(event, "confirm", "")
	if event.Frame.View.Menu.Choices[0].Display != "1  empty" {
		t.Fatalf("the native deletion must empty the displayed slot: %+v", event.Frame.View.Menu)
	}
	server.mu.Lock()
	defer server.mu.Unlock()
	writes := server.params["thread/presets/write"]
	if len(writes) != 3 || stored.Input != "" ||
		len(server.params["config/read"]) != 0 || len(server.params["config/value/write"]) != 0 ||
		len(server.params["turn/start"]) != 0 {
		t.Fatalf("unexpected preset write or submission: %s", writes)
	}
}

func TestDirectNativeSaveDraftIsUpHistoryOnNextAttachment(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "history.sock")
	server := startMockNativeAppServer(t, socket)
	var history []jsontext.Value
	server.overrides["thread/inputHistory/read"] = func(jsontext.Value) any {
		return map[string]any{"data": history}
	}
	server.overrides["thread/inputHistory/append"] = func(raw jsontext.Value) any {
		var draft struct {
			ThreadID string `json:"threadId"`
			Text     string `json:"text"`
		}
		if err := json.Unmarshal(raw, &draft); err != nil {
			return mockRPCError(err.Error())
		}
		entry, _ := json.Marshal(map[string]string{"sessionId": draft.ThreadID, "text": draft.Text})
		history = append(history, entry)
		return map[string]any{}
	}
	ctx := t.Context()
	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	client := &nativeClient{connection: conn}
	client.event = awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool { return event.Frame.Facts.HistoryRevision != 0 })
	client.editor("main").Replace(composer.Draft{Text: "unsent draft"})
	client.flushNativeView()
	if client.notice != "" {
		t.Fatal(client.notice)
	}
	_ = conn.Close()
	conn, err = openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	views, err := composition.NativeRenderer(false)
	if err != nil {
		t.Fatal(err)
	}
	terminal, _ := views.NewAttachment(diffrender.DarkTheme)
	client = &nativeClient{connection: conn, renderer: nativeClientRenderer{terminal: terminal, views: views}}
	client.accept(awaitDirectEvent(t, conn, func(event sessionapi.NativeEvent) bool { return len(event.Frame.Facts.History) == 1 }))
	if text := client.editor("main").Text; text != "" {
		t.Fatalf("saved draft was prefilled: %q", text)
	}
	client.dispatch(terminalsession.Input{Kind: terminalsession.KeyInput, Text: "\x1b[A"})
	if text := client.editor("main").Text; text != "unsent draft" {
		t.Fatalf("Up did not recall saved draft: %q", text)
	}
}
