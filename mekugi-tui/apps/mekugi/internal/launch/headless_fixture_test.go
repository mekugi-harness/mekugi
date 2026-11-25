package launch

import (
	"bufio"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

func TestAutoWrapProcess(t *testing.T) {
	if os.Getenv("MEKUGI_AUTO_WRAP_PROCESS") != "1" {
		return
	}
	if os.Args[3] == "router" {
		var args []string
		if os.Getenv("MEKUGI_AUTO_WRAP_TERMINAL") == "1" {
			args = []string{"--yolo"}
		}
		if os.Getenv("MEKUGI_AUTO_WRAP_HEADLESS") == "1" {
			args = []string{"headless", "--yolo"}
			if trust := os.Getenv("MEKUGI_AUTO_WRAP_TRUST"); trust != "" {
				args = append(args, "-c", "bypass_hook_trust="+trust)
			}
		}
		var routerArgs []string
		code, err := wrapCodex(t.Context(), routerArgs, args)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
		}
		os.Exit(code)
	}
	if len(os.Args) > 3 && os.Args[3] == "codex" {
		scenario := os.Getenv("MEKUGI_AUTO_WRAP_HEADLESS_SCENARIO")
		headlessHost := os.Getenv("MEKUGI_AUTO_WRAP_HEADLESS") == "1"
		runtimeStarted := make(chan struct{})
		markerPath := os.Getenv("MEKUGI_AUTO_WRAP_RPC_MARKER")
		if argsMarker := os.Getenv("MEKUGI_AUTO_WRAP_ARGS_MARKER"); argsMarker != "" {
			if err := os.WriteFile(argsMarker, []byte(strings.Join(os.Args[4:], "\n")+"\n"), 0o600); err != nil {
				t.Fatal(err)
			}
		}
		marker, err := os.OpenFile(markerPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
		if err != nil {
			t.Fatal(err)
		}
		defer marker.Close()
		if headlessHost {
			interrupt := make(chan os.Signal, 1)
			signal.Notify(interrupt, os.Interrupt)
			go func() {
				<-interrupt
				_ = os.WriteFile(markerPath+".joined", []byte("joined"), 0600)
				os.Exit(0)
			}()
		}
		workStatus := ""
		childStatus := ""
		subtreeReads := 0
		scanner := bufio.NewScanner(os.Stdin)
		for scanner.Scan() {
			var request struct {
				ID     jsontext.Value `json:"id"`
				Method string         `json:"method"`
				Params struct {
					Cwds   []string       `json:"cwds"`
					Thread string         `json:"threadId"`
					Config map[string]any `json:"config"`
					Input  []struct {
						Type string `json:"type"`
						Text string `json:"text"`
					} `json:"input"`
				} `json:"params"`
			}
			if err := json.Unmarshal(scanner.Bytes(), &request); err != nil {
				t.Fatal(err)
			}
			fmt.Fprintln(marker, request.Method)
			switch request.Method {
			case "initialize":
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{}}`+"\n", request.ID)
			case "config/read":
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{"config":{"compact_prompt":"Fixture caller handover."},"layers":[{"name":{"type":"sessionFlags"},"config":{"hooks":{"PreToolUse":[],"state":{}}}}]}}`+"\n", request.ID)
			case "hooks/list":
				if request.Params.Thread == "auto-wrap" {
					request.Params.Cwds = []string{os.Getenv("MEKUGI_AUTO_WRAP_WORKSPACE")}
				}
				if len(request.Params.Cwds) != 1 {
					t.Fatal("hook discovery did not select one absolute workspace")
				}
				var hooks []map[string]any
				if os.Getenv("MEKUGI_AUTO_WRAP_HOOK_CHOICE") != "" {
					hooks = append(hooks, map[string]any{"key": "caller-modified", "eventName": "sessionStart", "enabled": true, "trustStatus": "modified", "handlerType": "command", "command": "bash scripts/startup.sh", "sourcePath": request.Params.Cwds[0] + "/.codex/hooks.json", "currentHash": "sha256:modified"})
				}
				result, err := json.Marshal(map[string]any{"data": []any{map[string]any{"cwd": request.Params.Cwds[0], "errors": []any{}, "hooks": hooks}}})
				if err != nil {
					t.Fatal(err)
				}
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":%s}`+"\n", request.ID, result)
			case "config/batchWrite":
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{}}`+"\n", request.ID)
			case "model/list":
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{"data":[]}}`+"\n", request.ID)
			case "thread/subtree/list":
				subtreeReads++
				if scenario == "subtree-retry" && subtreeReads == 1 {
					fmt.Fprintf(os.Stdout, `{"id":%s,"error":{"code":-32000,"message":"transient subtree error"}}`+"\n", request.ID)
					continue
				}
				agent := binding.ThreadInfo{ID: "auto-wrap", Source: []byte(`{"subAgent":{"thread_spawn":{"agent_path":"/root"}}}`), CanAcceptDirectInput: new(true)}
				if workStatus != "" {
					agent.AgentStatus, _ = json.Marshal(workStatus)
				}
				members := []binding.ThreadInfo{agent}
				if scenario == "family-join" && childStatus != "" {
					if subtreeReads >= 2 {
						childStatus = "completed"
						fmt.Fprintln(os.Stdout, `{"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn","status":"completed"}}}`)
					}
					child := binding.ThreadInfo{ID: "child-1", ParentThreadID: "auto-wrap", Source: []byte(`{"subAgent":{"thread_spawn":{"agent_path":"/root/worker"}}}`)}
					child.AgentStatus, _ = json.Marshal(childStatus)
					members = append(members, child)
				}
				if scenario == "late-delegate-event" && childStatus != "" {
					child := binding.ThreadInfo{ID: "child-1", ParentThreadID: "auto-wrap", Source: []byte(`{"subAgent":{"thread_spawn":{"agent_path":"/root/worker"}}}`)}
					child.AgentStatus, _ = json.Marshal("completed")
					members = append(members, child)
				}
				response, err := json.Marshal(map[string]any{"id": request.ID, "result": map[string]any{"data": members}})
				if err != nil {
					t.Fatal(err)
				}
				fmt.Fprintln(os.Stdout, string(response))
			case "environment/info":
				close(runtimeStarted)
				if headlessHost && scenario == "startup" {
					fmt.Fprintln(os.Stderr, "fixture: invalid model configuration")
					os.Exit(2)
				}
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{"shell":{"name":"bash","path":"/bin/bash"}}}`+"\n", request.ID)
			case "thread/start":
				if trust := os.Getenv("MEKUGI_AUTO_WRAP_TRUST"); trust != "" && request.Params.Config["bypass_hook_trust"] != (trust == "true") {
					t.Fatal("thread/start omitted explicit hook trust")
				}
				for _, arg := range os.Args {
					if strings.HasPrefix(arg, "experimental_compact_prompt_file=") || strings.HasPrefix(arg, "compact_prompt=") {
						t.Fatalf("launch injected a compaction prompt: %s", arg)
					}
				}
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{"thread":{"id":"auto-wrap","cwd":%q},"model":"test-model"}}`+"\n", request.ID, os.Getenv("MEKUGI_AUTO_WRAP_WORKSPACE"))
				if os.Getenv("MEKUGI_AUTO_WRAP_FAILURE") == "1" {
					time.Sleep(100 * time.Millisecond)
					os.Exit(23)
				}
			case "turn/start":
				switch scenario {
				case "rpc-rejected":
					fmt.Fprintf(os.Stdout, `{"id":%s,"error":{"code":-1,"message":"fixture: start rejected"}}`+"\n", request.ID)
					continue
				case "rpc-stale":
					fmt.Fprintf(os.Stdout, `{"id":%q,"error":{"code":-1,"message":"fixture: stale rejection"}}`+"\n", string(request.ID))
				}
				switch scenario {
				case "approval":
					fmt.Fprintln(os.Stdout, `{"id":"approval","method":"item/commandExecution/requestApproval","params":{"threadId":"auto-wrap"}}`)
					continue
				case "shell-guard":
					fmt.Fprintln(os.Stdout, `{"id":"guard-1","method":"shellGuard/requestApproval","params":{"threadId":"auto-wrap","command":"git push"}}`)
					continue
				case "question":
					fmt.Fprintln(os.Stdout, `{"method":"item/completed","params":{"threadId":"auto-wrap","item":{"id":"question","type":"agentMessage","delivery":"async","questions":[{"id":"q"}]}}}`)
					continue
				case "disconnect":
					fmt.Fprintln(os.Stderr, "fixture: host transport ended")
					os.Exit(23)
				}
				if prompt := os.Getenv("MEKUGI_AUTO_WRAP_PROMPT"); prompt != "" && (len(request.Params.Input) != 1 || request.Params.Input[0].Text != prompt) {
					t.Fatal("literal stdin prompt was changed")
				}
				fmt.Fprintf(os.Stdout, `{"id":%s,"result":{"turn":{"id":"test-turn"}}}`+"\n", request.ID)
				fmt.Fprintln(os.Stdout, `{"method":"turn/started","params":{"threadId":"auto-wrap","turn":{"id":"test-turn"}}}`)
				if scenario == "family-join" {
					childStatus = "running"
					fmt.Fprintln(os.Stdout, `{"method":"thread/started","params":{"thread":{"id":"child-1","parentThreadId":"auto-wrap"}}}`)
				}
				if scenario == "late-delegate-event" {
					childStatus = "completed"
					fmt.Fprintln(os.Stdout, `{"method":"thread/started","params":{"thread":{"id":"child-1","parentThreadId":"auto-wrap"}}}`)
				}
				status := "completed"
				if scenario == "failed" || scenario == "interrupted" {
					status = scenario
				}
				fmt.Fprintf(os.Stdout, `{"method":"turn/completed","params":{"threadId":"auto-wrap","turn":{"id":"test-turn","status":%q}}}`+"\n", status)
				workStatus = status
				if status == "failed" {
					workStatus = "errored"
				}
			}
		}
		if err := scanner.Err(); err != nil {
			t.Fatal(err)
		}
		if headlessHost {
			if scenario == "late-delegate-event" {
				// Emit the delegate's final turn/completed notification after the
				// terminal subtree snapshot was already returned, during EOF flush.
				fmt.Fprintln(os.Stdout, `{"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn","status":"completed"}}}`)
			}
			select {
			case <-runtimeStarted:
				if err := os.WriteFile(markerPath+".joined", []byte("joined"), 0600); err != nil {
					t.Fatal(err)
				}
			default:
			}
		}
		os.Exit(0) // Keep the test runner's PASS line out of the RPC stream.
	}

}

