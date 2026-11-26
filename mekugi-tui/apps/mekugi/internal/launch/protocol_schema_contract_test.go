package launch

import (
	json "encoding/json/v2"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"
)

type schemaEnvelope struct {
	OneOf []struct {
		Properties map[string]struct {
			Enum  []string `json:"enum"`
			Const string   `json:"const"`
		} `json:"properties"`
	} `json:"oneOf"`
}

func loadSchemaMethods(t *testing.T, schemaDir, filename string) map[string]bool {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(schemaDir, filename))
	if err != nil {
		t.Fatalf("read %s: %v", filename, err)
	}
	var doc schemaEnvelope
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatalf("parse %s: %v", filename, err)
	}
	methods := make(map[string]bool, len(doc.OneOf))
	for _, variant := range doc.OneOf {
		prop, ok := variant.Properties["method"]
		if !ok {
			continue
		}
		if prop.Const != "" {
			methods[prop.Const] = true
		}
		for _, m := range prop.Enum {
			methods[m] = true
		}
	}
	if len(methods) == 0 {
		t.Fatalf("no methods found in %s", filename)
	}
	return methods
}

func repoRootFromTest(t *testing.T) string {
	t.Helper()
	dir, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "codex-rs", "app-server-protocol", "schema", "json")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatalf("could not locate repo root from %s", dir)
		}
		dir = parent
	}
}

func TestGoNativeProtocolMethodsMatchRustSchema(t *testing.T) {
	root := repoRootFromTest(t)
	schemaDir := filepath.Join(root, "codex-rs", "app-server-protocol", "schema", "json")
	clientRequests := loadSchemaMethods(t, schemaDir, "ClientRequest.json")
	// Also admit experimental ClientRequest methods declared in protocol/common.rs.
	commonSrc, err := os.ReadFile(filepath.Join(root, "codex-rs", "app-server-protocol", "src", "protocol", "common.rs"))
	if err != nil {
		t.Fatalf("read protocol/common.rs: %v", err)
	}
	expRe := regexp.MustCompile(`#\[experimental\("([a-zA-Z0-9_/]+)"\)\]`)
	for _, match := range expRe.FindAllStringSubmatch(string(commonSrc), -1) {
		clientRequests[match[1]] = true
	}
	serverNotifications := loadSchemaMethods(t, schemaDir, "ServerNotification.json")
	serverRequests := loadSchemaMethods(t, schemaDir, "ServerRequest.json")

	launchDir := filepath.Join(root, "mekugi-tui", "apps", "mekugi", "internal", "launch")
	callRe := regexp.MustCompile(`\.Call\(\s*[^,]+,\s*"([a-zA-Z0-9_/]+)"`)
	caseRe := regexp.MustCompile(`case\s+([^:]+):`)
	quotedMethodRe := regexp.MustCompile(`"([a-zA-Z0-9_]+/[a-zA-Z0-9_/]+)"`)

	clientFiles := []string{
		"direct_native_connection.go",
		"direct_native_operations.go",
		"direct_native_projection.go",
		"native_app_server_wire.go",
	}
	checkedCalls := make(map[string]bool)
	for _, name := range clientFiles {
		src, err := os.ReadFile(filepath.Join(launchDir, name))
		if err != nil {
			t.Fatalf("read %s: %v", name, err)
		}
		for _, match := range callRe.FindAllStringSubmatch(string(src), -1) {
			method := match[1]
			if method == "initialize" {
				continue
			}
			checkedCalls[method] = true
			if !clientRequests[method] {
				t.Errorf("%s calls unknown ClientRequest method %q (not in ClientRequest.json)", name, method)
			}
		}
	}
	if len(checkedCalls) == 0 {
		t.Fatal("expected to verify at least one wire.Call method")
	}

	projSrc, err := os.ReadFile(filepath.Join(launchDir, "direct_native_projection.go"))
	if err != nil {
		t.Fatalf("read direct_native_projection.go: %v", err)
	}
	checkedIncoming := make(map[string]bool)
	for _, match := range caseRe.FindAllStringSubmatch(string(projSrc), -1) {
		for _, q := range quotedMethodRe.FindAllStringSubmatch(match[1], -1) {
			method := q[1]
			if strings.HasPrefix(method, "image/") || strings.HasPrefix(method, "text/") || strings.HasPrefix(method, "application/") {
				continue
			}
			checkedIncoming[method] = true
			if !serverNotifications[method] && !serverRequests[method] {
				t.Errorf("direct_native_projection.go handles unknown server method %q (not in ServerNotification.json or ServerRequest.json)", method)
			}
		}
	}
	if len(checkedIncoming) == 0 {
		t.Fatal("expected to verify at least one server notification/request method")
	}

	sortedCalls := make([]string, 0, len(checkedCalls))
	for m := range checkedCalls {
		sortedCalls = append(sortedCalls, m)
	}
	sort.Strings(sortedCalls)
	t.Logf("verified %d ClientRequest methods and %d server notification/request methods against codex-rs/app-server-protocol/schema/json", len(sortedCalls), len(checkedIncoming))
}
