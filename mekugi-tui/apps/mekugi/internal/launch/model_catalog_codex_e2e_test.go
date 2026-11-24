//go:build codex_e2e

package launch

import (
	"context"
	"encoding/base64"
	json "encoding/json/v2"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/nativetrace"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
	segmenttransport "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/transport"
)

// Native Codex fetches the built-in provider's catalog and executes stock shell,
// patch, and web-search tools against the same local fixture.
func TestCodeModeOnlyCatalogNativeCodex(t *testing.T) {
	codex, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 90*time.Second)
	defer cancel()
	environment := builtInOpenAICodexEnvironment(t)
	// ChatGPT discovery follows the invocation's local OpenAI URL. API-key
	// discovery deliberately skips overridden base URLs in stock Codex.
	var configRoot string
	for _, entry := range environment {
		if value, ok := strings.CutPrefix(entry, "CODEX_HOME="); ok {
			configRoot = value
		}
	}
	claims := `{"email":"fixture@example.test","https://api.openai.com/auth":{"chatgpt_account_id":"fixture-account","chatgpt_plan_type":"pro"}}`
	token := "eyJhbGciOiJub25lIn0." + base64.RawURLEncoding.EncodeToString([]byte(claims)) + ".c2ln"
	auth, err := json.Marshal(map[string]any{
		"tokens":       map[string]string{"id_token": token, "access_token": "fixture-access", "refresh_token": "fixture-refresh", "account_id": "fixture-account"},
		"last_refresh": time.Now().UTC().Format(time.RFC3339),
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(configRoot, "auth.json"), auth, 0600); err != nil {
		t.Fatal(err)
	}
	body, err := bundledModelCatalog(ctx, codex)
	if err != nil {
		t.Fatal(err)
	}
	var discoveries, searches atomic.Int32
	workspace := t.TempDir()
	proxy := newManagedMekugiProxy(t)
	traceRoot := t.TempDir()
	proxy.nativeTrace = nativetrace.NewWithNotice(traceRoot, func(message string) { t.Error(message) })
	const patch = "*** Begin Patch\n*** Add File: trace-effect.txt\n+trace effect\n*** End Patch"
	program := `text(await tools.exec_command({cmd:"printf MODE_OK"})); await tools.apply_patch(` +
		string(responses.MustMarshalJSON(patch)) + `); text(await tools.web__run({search_query:[{q:"fixture lookup"}]}));`
	traceConfirmed := false
	fixture := &toolFrontendCodexProvider{
		program:  program,
		expected: []string{"MODE_OK", "MEKUGI_WEB_SEARCH_FIXTURE_RESULT"}, finalText: "catalog host accepted",
		observeRequest: func(body []byte, headers http.Header) error {
			type toolDefinition struct {
				Type        string           `json:"type"`
				Name        string           `json:"name"`
				Description string           `json:"description"`
				Tools       []toolDefinition `json:"tools"`
			}
			var request struct {
				Instructions string           `json:"instructions"`
				Tools        []toolDefinition `json:"tools"`
				Input        []struct {
					Type   string           `json:"type"`
					CallID string           `json:"call_id"`
					Tools  []toolDefinition `json:"tools"`
				} `json:"input"`
			}
			if err := json.Unmarshal(body, &request); err != nil {
				return err
			}
			for _, item := range request.Input {
				if item.Type != "custom_tool_call_output" || item.CallID != "frontend-call" {
					continue
				}
				thread := headers.Get(fixtureThreadIDHeader)
				if thread == "" {
					metadata, _ := fixtureTurnMetadata(headers)
					thread = metadata.ThreadID
				}
				cell := proxy.nativeTrace.ReadCell(thread, "frontend-call", program)
				commands, ok := cell.Commands([]commandanalysis.Command{{Command: "printf MODE_OK"}}, workspace)
				if !ok || len(commands) != 1 || commands[0].ExitCode == nil || *commands[0].ExitCode != 0 {
					return fmt.Errorf("native trace did not confirm nested shell completion: %+v", commands)
				}
				if result, ok := cell.Patch(patch, 0); !ok || result.Status != "completed" {
					return fmt.Errorf("native trace did not confirm nested patch completion: %+v", result)
				}
				traceConfirmed = true
			}
			const helper = "mchanges --list"
			if strings.Contains(request.Instructions, helper) {
				return fmt.Errorf("native request put shell helper guidance in base instructions")
			}
			var execTools, descriptions int
			var checkTools func([]toolDefinition) error
			checkTools = func(tools []toolDefinition) error {
				for _, tool := range tools {
					switch tool.Name {
					case "exec":
						if tool.Type == "custom" {
							execTools++
							if strings.Contains(tool.Description, helper) {
								descriptions++
							}
						}
					case "apply_patch", "exec_command":
						return fmt.Errorf("native request advertised the separate %s tool", tool.Name)
					}
					if err := checkTools(tool.Tools); err != nil {
						return err
					}
				}
				return nil
			}
			if err := checkTools(request.Tools); err != nil {
				return err
			}
			for _, item := range request.Input {
				if item.Type == "additional_tools" {
					if err := checkTools(item.Tools); err != nil {
						return err
					}
				}
			}
			if execTools != 1 || descriptions != 1 {
				return fmt.Errorf("native catalog has %d exec tools, %d with selected helper guidance", execTools, descriptions)
			}
			return nil
		},
	}
	attachTestReplayStore(t, proxy)
	inference := fixtureResponsesHandler(fixture)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/alpha/search":
			searches.Add(1)
			var request struct {
				Commands struct {
					SearchQuery []struct {
						Query string `json:"q"`
					} `json:"search_query"`
				} `json:"commands"`
			}
			if err := json.UnmarshalRead(r.Body, &request); err != nil || r.Method != http.MethodPost || len(request.Commands.SearchQuery) != 1 || request.Commands.SearchQuery[0].Query != "fixture lookup" {
				t.Errorf("stock web search request changed: %+v, error=%v", request, err)
			}
			if r.Header.Get("Authorization") != "Bearer fixture-access" || r.Header.Get("Chatgpt-Account-Id") != "fixture-account" {
				t.Error("stock web search lost host authentication")
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"output":"MEKUGI_WEB_SEARCH_FIXTURE_RESULT"}`))
		case "/v1/models":
			if r.Header.Get("Authorization") != "Bearer fixture-access" || r.Header.Get("Chatgpt-Account-Id") != "fixture-account" {
				t.Error("model catalog lost native account authentication")
			}
			discoveries.Add(1)
			w.Header().Set("Content-Type", "application/json")
			w.Header().Set("ETag", "mixed-account-catalog")
			_, _ = w.Write(body)
		case "/v1/responses":
			if r.Method == http.MethodGet {
				http.Error(w, "fixture requires HTTP", http.StatusUpgradeRequired)
				return
			}
			inference.ServeHTTP(w, r)
		default:
			t.Errorf("unexpected provider request: %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer server.Close()
	args := fixtureProviderArgs(server.URL+"/v1", []string{
		"-c", `model="gpt-6-astra"`, "-c", `approval_policy="never"`, "-c", `sandbox_mode="danger-full-access"`,
		"-c", "features.plugins=false",
		"-c", `web_search="live"`,
		"-c", "chatgpt_base_url=" + strconv.Quote(server.URL),
		"-c", "model_reasoning_effort=" + strconv.Quote("low"),
	}, "")
	environment, err = frontendShellEnvironment(environment, proxy.registry.FrontendDirectory, "", segmenttransport.Endpoints{})
	if err != nil {
		t.Fatal(err)
	}
	environment = append(environment, "CODEX_ROLLOUT_TRACE_ROOT="+traceRoot)
	terminal := startAppResumeTerminalWithIntegration(t, func(runCtx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(runCtx, codex, nativeAppServerArgs(args)...)
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}, "", testAppServerIntegration(proxy))
	terminal.await("Ready")
	terminal.send("Run the execution interface fixture.\r")
	terminal.await("catalog host accepted")
	terminal.await("completed")
	fixture.mu.Lock()
	defer fixture.mu.Unlock()
	if discoveries.Load() == 0 || searches.Load() != 1 || !fixture.resultSeen || fixture.turns != 2 {
		t.Fatalf("catalog/host execution: discoveries=%d searches=%d result=%t turns=%d",
			discoveries.Load(), searches.Load(), fixture.resultSeen, fixture.turns)
	}
	if !traceConfirmed {
		t.Fatal("provider continuation did not check live native trace evidence")
	}
}
