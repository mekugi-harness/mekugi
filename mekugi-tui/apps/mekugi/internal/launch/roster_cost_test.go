package launch

import (
	"math"
	"testing"

	"github.com/charmbracelet/x/ansi"

	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

func notifyTestProviderUsage(t *testing.T, u *appServerUI, thread, response, model string, openai bool, input, output uint64) {
	t.Helper()
	appServerTestNotify(t, u, "rawResponse/completed", testProviderUsage(thread, response, model, openai, input, output))
}

func testProviderUsage(thread, response, model string, openai bool, input, output uint64) map[string]any {
	return map[string]any{
		"threadId": thread, "turnId": "turn", "responseId": response,
		"model": model, "openaiEndpoint": openai,
		"usage": map[string]any{"inputTokens": input, "outputTokens": output},
		"usageMetadata": map[string]any{"metadata": map[string]any{
			"input_tokens_details":  map[string]any{"cached_tokens": 0},
			"output_tokens_details": map[string]any{"reasoning_tokens": 0},
		}},
	}
}

func TestNativeRosterCostMatchesCanonicalUsage(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	setTestAppServerIntegration(u, newManagedMekugiProxy(t))
	appServerTestMessage(t, u, `{"method":"turn/started","params":{"threadId":"child","turn":{"id":"turn"}}}`)
	notifyTestProviderUsage(t, u, "child", "one", "gpt-6.2-sol", true, 100_000, 10_000)
	agent := u.attached.session.Agent(u.attached.session.KnownCaller("child"))
	if !agent.CostKnown || agent.CostPartial || math.Abs(agent.Cost-.3) > 1e-10 ||
		agent.InputTokens != 100_000 || agent.OutputTokens != 10_000 {
		t.Fatalf("native agent = %+v", agent)
	}
	if got := activityui.Cost(testActivityAgent(*agent)); got != "$0.30" {
		t.Fatalf("cost = %q", got)
	}
	root := u.attached.session.Agent("/root")
	if root.CostKnown || root.Cost != 0 {
		t.Fatalf("child cost leaked into Main: %+v", root)
	}
}

func TestNativeRosterUsageGapKeepsMeasuredCost(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	setTestAppServerIntegration(u, newManagedMekugiProxy(t))
	appServerTestMessage(t, u, `{"method":"turn/started","params":{"threadId":"child","turn":{"id":"turn"}}}`)
	notifyTestProviderUsage(t, u, "child", "one", "gpt-6.2-sol", true, 100_000, 10_000)
	appServerTestNotify(t, u, "rawResponse/completed", map[string]any{"threadId": "child", "responseId": "missing"})
	notifyTestProviderUsage(t, u, "child", "two", "gpt-6.2-sol", true, 100_000, 10_000)
	agent := u.attached.session.Agent(u.attached.session.KnownCaller("child"))
	if !agent.CostPartial || !agent.CostKnown || !agent.UsagePartial || !agent.TokensKnown || agent.InputTokens != 200_000 || agent.OutputTokens != 20_000 {
		t.Fatalf("missing usage hid measured counts or priced subtotal: %+v", agent)
	}
	if got := activityui.Cost(testActivityAgent(*agent)); got != "≥$0.60" {
		t.Fatalf("lower-bound cost = %q", got)
	}
	// T+N counts provider roundtrips, including one without usage, not Codex turns.
	if agent.Roundtrips != 3 || ansi.Strip(activityui.Turns(testActivityAgent(*agent))) != "T+3" {
		t.Fatalf("roundtrips = %d, label %q", agent.Roundtrips, activityui.Turns(testActivityAgent(*agent)))
	}
}

func TestNativeRosterUnknownPriceDoesNotClaimCost(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	setTestAppServerIntegration(u, newManagedMekugiProxy(t))
	appServerTestMessage(t, u, `{"method":"turn/started","params":{"threadId":"child","turn":{"id":"turn"}}}`)
	notifyTestProviderUsage(t, u, "child", "one", "gpt-6.2-sol", false, 10, 1)
	agent := u.attached.session.Agent(u.attached.session.KnownCaller("child"))
	if !agent.TokensKnown || agent.CostKnown || activityui.Cost(testActivityAgent(*agent)) != "" {
		t.Fatalf("unpriced provider usage: %+v", agent)
	}
}
