//go:build codex_e2e

package launch

import (
	"context"
	"net/http/httptest"
	"os/exec"
	"strconv"
	"strings"
	"testing"
)

func TestAppServerSessionTitleManualNativeCodexE2E(t *testing.T) {
	codex, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	provider := &appResumeProvider{}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()
	environment, workspace := routerFaultCodexEnvironment(t), t.TempDir()
	command := func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex,
			"-c", `model_providers.preview={name="preview",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false}`,
			"-c", `model_provider="preview"`, "-c", `model="gpt-6-astra"`, "-c", "features.plugins=false")
		cmd.Env, cmd.Dir = append(environment, "MEKUGI_AUTO_TITLE=1"), workspace
		return cmd
	}
	const title = "My manually renamed session"
	showsMainTitle := func(frame string) bool {
		return strings.Contains(frame, "Main • "+title+" •")
	}
	first := startAppResumeTerminal(t, command, "")
	first.awaitReady()
	first.send("/title " + title + "\r")
	first.awaitMatch("manual Main title before first prompt", showsMainTitle)
	if len(provider.snapshot()) != 0 {
		t.Fatal("manual title command reached the model")
	}
	first.send("Explain native session naming\r")
	first.await("Recovered after a retry.")
	threads := provider.snapshot()
	if len(threads) != 1 || threads[0] == "" {
		t.Fatalf("manual title should leave one ordinary native request, got threads %q", threads)
	}
	first.awaitThreadIdle(threads[0])
	first.quit()
	second := startAppResumeTerminal(t, command, threads[0])
	second.awaitReady()
	second.awaitMatch("persisted title after fresh app-server resume", showsMainTitle)
	second.send("Continue after manual rename\r")
	second.await("Continue after manual rename")
	second.awaitThreadIdle(threads[0])
	second.quit()
	if len(provider.snapshot()) != 2 {
		t.Fatal("resuming a manually named thread should only send the ordinary turn")
	}
}
