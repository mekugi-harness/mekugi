package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"

	json "encoding/json/v2"
	"fmt"
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
)

func TestUISnapshotNativeConfiguredServiceTier(t *testing.T) {
	directory := t.TempDir()
	u := newAppServerSessionTestUI(t, directory)
	proxy := newManagedMekugiProxy(t)
	setTestAppServerIntegration(u, proxy)
	u.attached.presentation.Status.Text = "Ready"
	u.attached.cwd = "/workspace"
	u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
	for _, tc := range []struct{ model, requested, want string }{
		{"gpt-6.1-sol", "priority", "priority"},
		{"gpt-6-astra", "flex", "flex"},
	} {
		t.Run(tc.model+"-"+tc.requested, func(t *testing.T) {
			appServerTestNotify(t, u, "thread/settings/updated", map[string]any{"threadId": "main", "threadSettings": map[string]any{"model": tc.model, "effort": "high", "serviceTier": tc.requested}})
			caption, _ := u.testStatLines(t, 100, 12)
			if frame := ansi.Strip(caption); !strings.Contains(frame, tc.model+" (high) · "+tc.want) {
				t.Fatalf("composer does not reflect routed tier: %s", frame)
			}
			if tc.model == "gpt-6.1-sol" {
				assertNativeUISnapshot(t, "native-main-configured-tier", []string{caption})
			}
			// Passthrough routes tiers too, but has no observation proxy.
			setTestAppServerIntegration(u, nil)
			if passthrough, _ := u.testStatLines(t, 100, 12); passthrough != caption {
				t.Fatal("passthrough composer lost configured tier")
			}
			appServerTestKeys(t, u, "/status\r")
			if body := statusReportText(u.statusPanel); !strings.Contains(body, "Service tier: "+tc.want) {
				t.Fatalf("status does not reflect routed tier: %s", body)
			}
			u.statusPanelKey("\x1b")
			setTestAppServerIntegration(u, proxy)
			if u.settings.Tier != tc.requested || u.settings.Model != tc.model {
				t.Fatal("presentation replaced host settings")
			}
		})
	}
}

func newServiceTierPickerTestUI(t *testing.T, model, tier string) (*appServerUI, *appServerTestInput) {
	t.Helper()
	u, input := newAppServerTestUI()
	u.ctx = t.Context()
	u.settings.Model, u.settings.Tier = model, tier
	u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
	if err := json.Unmarshal([]byte(`[{"model":"gpt-6.1-sol","serviceTiers":[{"id":"priority"},{"id":"flex"}]},{"model":"gpt-5.6-terra","serviceTiers":[{"id":"priority"},{"id":"flex"}]},{"model":"unconfigured","serviceTiers":[{"id":"priority"},{"id":"flex"}]}]`), &u.models); err != nil {
		t.Fatal(err)
	}
	return u, input
}

func TestUISnapshotNativeTierPicker(t *testing.T) {
	for _, tc := range []struct {
		name, model, hostTier string
		selected              int
	}{
		{"configured", "gpt-6.1-sol", "priority", 1},
		{"unconfigured", "unconfigured", "priority", 1},
		{"configured-flex", "gpt-6.1-sol", "flex", 2},
	} {
		for _, width := range []int{36, 100} {
			t.Run(fmt.Sprintf("%s/%d", tc.name, width), func(t *testing.T) {
				u, input := newServiceTierPickerTestUI(t, tc.model, tc.hostTier)
				appServerTestKeys(t, u, "/tier\r")
				settleTestPresentation(&u.attached, u.shell, &u.Client, &u.picker, &u.Editor, u.settings, u.title, u.backendVersion, u.restoring != nil, u.side, u.clock, &u.questions, &u.approvals, u.statusPanel != nil || u.resumePicker != nil)
				if !u.picker.Open || u.picker.Setting != "/tier" || u.shell.Main.Menu.Selected != tc.selected {
					t.Fatalf("picker did not select effective tier: %+v", u.picker)
				}
				if input.Len() != 0 || u.settings.Tier != tc.hostTier || u.settings.Model != tc.model {
					t.Fatal("opening picker changed host settings")
				}
				assertNativeUISnapshot(t, fmt.Sprintf("native-tier-%s-%d", tc.name, width), u.renderPicker(width, 7))
			})
		}
	}
}

func TestUISnapshotNativeTierPickerModelSwitch(t *testing.T) {
	u, _ := newServiceTierPickerTestUI(t, "gpt-6.1-sol", "default")
	appServerTestKeys(t, u, "/tier\r")
	appServerTestKeys(t, u, "\x1b")
	appServerTestNotify(t, u, "thread/settings/updated", map[string]any{"threadId": "main", "threadSettings": map[string]any{"model": "unconfigured", "effort": "high", "serviceTier": "priority"}})
	appServerTestKeys(t, u, "/tier\r")
	settleTestPresentation(&u.attached, u.shell, &u.Client, &u.picker, &u.Editor, u.settings, u.title, u.backendVersion, u.restoring != nil, u.side, u.clock, &u.questions, &u.approvals, u.statusPanel != nil || u.resumePicker != nil)
	if u.picker.Problem != "" || u.shell.Main.Menu.Selected != 1 || u.settings.Tier != "priority" || u.settings.Model != "unconfigured" {
		t.Fatalf("model switch retained override or lost host settings: picker=%+v model=%s tier=%s", u.picker, u.settings.Model, u.settings.Tier)
	}
	assertNativeUISnapshot(t, "native-tier-model-switch-100", u.renderPicker(100, 7))
}

func TestUISnapshotNativeTierPickerCatalogStates(t *testing.T) {
	for _, state := range []string{"loading", "failed"} {
		for _, width := range []int{36, 100} {
			t.Run(fmt.Sprintf("%s/%d", state, width), func(t *testing.T) {
				u, _ := newServiceTierPickerTestUI(t, "gpt-6.1-sol", "default")
				u.modelsLoading = true
				appServerTestRequest(t, u, "99", "model/list")
				appServerTestKeys(t, u, "/tier\r")
				if state == "failed" {
					appServerTestMessage(t, u, `{"id":99,"error":{"code":-1,"message":"catalog unavailable"}}`)
					if u.modelsLoading || u.picker.Loading || u.picker.Problem == "" {
						t.Fatal("catalog failure did not end loading and preserve its error")
					}
				} else if !u.picker.Loading {
					t.Fatal("picker did not expose catalog loading state")
				}
				assertNativeUISnapshot(t, fmt.Sprintf("native-tier-%s-%d", state, width), u.renderPicker(width, 7))
			})
		}
	}
}

func TestServiceTierPickerSelectionUpdatesEffectiveTier(t *testing.T) {
	for _, tc := range []struct {
		name, initial, keys string
		want                any
	}{
		{"flex", "", "\x1b[F\r", "flex"},
		{"default", "flex", "\x1b[H\r", nil},
		{"pin-default", "", "\x1b[H\r", nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			u, input := newServiceTierPickerTestUI(t, "gpt-6.1-sol", tc.initial)
			appServerTestKeys(t, u, "/tier\r"+tc.keys)
			assertAppServerSettingsRequest(t, input, "thread/settings/update", map[string]any{"threadId": "main", "serviceTier": tc.want})
			if u.picker.Open || !u.Settings.Pending() || u.settings.Tier != tc.initial {
				t.Fatal("selection replaced host settings before confirmation")
			}
			appServerTestNotify(t, u, "thread/settings/updated", map[string]any{"threadId": "main", "threadSettings": map[string]any{"model": "gpt-6.1-sol", "effort": "high", "serviceTier": tc.want}})
		})
	}
}
