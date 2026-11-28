package launch

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

type protocolSchemaEnvelope struct {
	OneOf []struct {
		Properties map[string]struct {
			Enum  []string `json:"enum"`
			Const string   `json:"const"`
		} `json:"properties"`
	} `json:"oneOf"`
}

type protocolSchemaDoc struct {
	Ref         string                    `json:"$ref"`
	Properties  map[string]jsontext.Value `json:"properties"`
	Definitions map[string]jsontext.Value `json:"definitions"`
	AllOf       []jsontext.Value          `json:"allOf"`
	AnyOf       []jsontext.Value          `json:"anyOf"`
	OneOf       []jsontext.Value          `json:"oneOf"`
}

type precomputedProtocolExports struct {
	JSONSchema map[string]string `json:"json_schema"`
	TypeScript map[string]string `json:"typescript"`
}

type launchProtocolSchemas struct {
	clientRequests      map[string]bool
	clientNotifications map[string]bool
	serverNotifications map[string]bool
	serverRequests      map[string]bool
	defs                map[string][]protocolSchemaDoc
}

func parseEnvelopeMethods(t *testing.T, label string, raw []byte) map[string]bool {
	t.Helper()
	var doc protocolSchemaEnvelope
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatalf("parse %s: %v", label, err)
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
		t.Fatalf("no methods found in %s", label)
	}
	return methods
}

func mergeMethodSet(dst, src map[string]bool) {
	for m := range src {
		dst[m] = true
	}
}

func loadLaunchProtocolSchemas(t *testing.T) *launchProtocolSchemas {
	t.Helper()
	schemas := &launchProtocolSchemas{
		clientRequests:      make(map[string]bool),
		clientNotifications: make(map[string]bool),
		serverNotifications: make(map[string]bool),
		serverRequests:      make(map[string]bool),
		defs:                make(map[string][]protocolSchemaDoc),
	}

	addBundle := func(label string, raw []byte) {
		var doc protocolSchemaDoc
		if err := json.Unmarshal(raw, &doc); err != nil {
			t.Fatalf("parse %s: %v", label, err)
		}
		for k, rawDef := range doc.Definitions {
			if len(rawDef) == 0 || rawDef[0] != '{' {
				continue
			}
			var def protocolSchemaDoc
			if err := json.Unmarshal(rawDef, &def); err != nil {
				t.Fatalf("parse %s definition %s: %v", label, k, err)
			}
			schemas.defs[k] = append(schemas.defs[k], def)
		}
	}

	// Read Rust schemas through package-local testdata symlinks so Go's test cache
	// tracks edits and works cleanly under -trimpath.
	stableDir := filepath.Join("testdata", "app-server-protocol-json")
	readStable := func(filename string) []byte {
		raw, err := os.ReadFile(filepath.Join(stableDir, filename))
		if err != nil {
			t.Fatalf("read stable %s: %v", filename, err)
		}
		return raw
	}
	mergeMethodSet(schemas.clientRequests, parseEnvelopeMethods(t, "stable/ClientRequest.json", readStable("ClientRequest.json")))
	mergeMethodSet(schemas.clientNotifications, parseEnvelopeMethods(t, "stable/ClientNotification.json", readStable("ClientNotification.json")))
	mergeMethodSet(schemas.serverNotifications, parseEnvelopeMethods(t, "stable/ServerNotification.json", readStable("ServerNotification.json")))
	mergeMethodSet(schemas.serverRequests, parseEnvelopeMethods(t, "stable/ServerRequest.json", readStable("ServerRequest.json")))
	addBundle("stable/codex_app_server_protocol.schemas.json", readStable("codex_app_server_protocol.schemas.json"))
	addBundle("stable/codex_app_server_protocol.v2.schemas.json", readStable("codex_app_server_protocol.v2.schemas.json"))

	expCompressed, err := os.ReadFile(filepath.Join("testdata", "app-server-exports-experimental.json.zst"))
	if err != nil {
		t.Fatalf("read experimental exports: %v", err)
	}
	dec, err := zstd.NewReader(nil)
	if err != nil {
		t.Fatalf("init zstd decoder: %v", err)
	}
	defer dec.Close()
	expRaw, err := dec.DecodeAll(expCompressed, nil)
	if err != nil {
		t.Fatalf("decompress experimental exports: %v", err)
	}
	var exp precomputedProtocolExports
	if err := json.Unmarshal(expRaw, &exp); err != nil {
		t.Fatalf("parse experimental exports: %v", err)
	}
	readExp := func(filename string) []byte {
		raw, ok := exp.JSONSchema[filename]
		if !ok {
			t.Fatalf("experimental exports missing json_schema/%s", filename)
		}
		return []byte(raw)
	}
	mergeMethodSet(schemas.clientRequests, parseEnvelopeMethods(t, "experimental/ClientRequest.json", readExp("ClientRequest.json")))
	mergeMethodSet(schemas.clientNotifications, parseEnvelopeMethods(t, "experimental/ClientNotification.json", readExp("ClientNotification.json")))
	mergeMethodSet(schemas.serverNotifications, parseEnvelopeMethods(t, "experimental/ServerNotification.json", readExp("ServerNotification.json")))
	mergeMethodSet(schemas.serverRequests, parseEnvelopeMethods(t, "experimental/ServerRequest.json", readExp("ServerRequest.json")))
	addBundle("experimental/codex_app_server_protocol.schemas.json", readExp("codex_app_server_protocol.schemas.json"))
	addBundle("experimental/codex_app_server_protocol.v2.schemas.json", readExp("codex_app_server_protocol.v2.schemas.json"))

	// Internal notifications excluded from ServerNotification.json (e.g. rawResponse/completed)
	// are still exported in the Rust-generated ServerNotification.ts union.
	tsNotifs, ok := exp.TypeScript["ServerNotification.ts"]
	if !ok {
		t.Fatal("experimental exports missing typescript/ServerNotification.ts")
	}
	tsMethodRe := regexp.MustCompile(`"method":\s*"([^"]+)"`)
	for _, match := range tsMethodRe.FindAllStringSubmatch(tsNotifs, -1) {
		schemas.serverNotifications[match[1]] = true
	}

	return schemas
}

func (s *launchProtocolSchemas) propertiesOf(t *testing.T, names ...string) map[string]bool {
	t.Helper()
	props := make(map[string]bool)
	visited := make(map[string]bool)

	var collect func(doc protocolSchemaDoc)
	collectRaw := func(raw jsontext.Value) {
		if len(raw) == 0 || raw[0] != '{' {
			return
		}
		var sub protocolSchemaDoc
		if err := json.Unmarshal(raw, &sub); err != nil {
			t.Fatalf("parse subschema: %v", err)
		}
		collect(sub)
	}
	collect = func(doc protocolSchemaDoc) {
		if doc.Ref != "" {
			if refName, ok := strings.CutPrefix(doc.Ref, "#/definitions/"); ok && !visited[refName] {
				visited[refName] = true
				for _, target := range s.defs[refName] {
					collect(target)
				}
			}
		}
		for k := range doc.Properties {
			props[k] = true
		}
		for _, sub := range doc.AllOf {
			collectRaw(sub)
		}
		for _, sub := range doc.AnyOf {
			collectRaw(sub)
		}
		for _, sub := range doc.OneOf {
			collectRaw(sub)
		}
	}

	for _, name := range names {
		docs := s.defs[name]
		if len(docs) == 0 {
			t.Fatalf("schema definition %q not found in Rust-generated JSON schemas", name)
		}
		for _, doc := range docs {
			collect(doc)
		}
	}
	return props
}

func unquoteStringLiteral(expr ast.Expr) (string, bool) {
	lit, ok := expr.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return "", false
	}
	val, err := strconv.Unquote(lit.Value)
	if err != nil {
		return "", false
	}
	return val, true
}

func resolveStringExprInFunc(fn *ast.FuncDecl, expr ast.Expr) []string {
	if val, ok := unquoteStringLiteral(expr); ok {
		return []string{val}
	}
	ident, ok := expr.(*ast.Ident)
	if !ok || fn == nil || fn.Body == nil {
		return nil
	}
	var values []string
	ast.Inspect(fn.Body, func(n ast.Node) bool {
		assign, ok := n.(*ast.AssignStmt)
		if !ok || len(assign.Lhs) != len(assign.Rhs) {
			return true
		}
		for i, lhs := range assign.Lhs {
			lhsIdent, ok := lhs.(*ast.Ident)
			if !ok || lhsIdent.Name != ident.Name {
				continue
			}
			if val, ok := unquoteStringLiteral(assign.Rhs[i]); ok {
				values = append(values, val)
			}
		}
		return true
	})
	return values
}

func TestGoNativeProtocolMethodsMatchRustSchema(t *testing.T) {
	schemas := loadLaunchProtocolSchemas(t)

	entries, err := os.ReadDir(".")
	if err != nil {
		t.Fatalf("read launch directory: %v", err)
	}

	fset := token.NewFileSet()
	checkedCalls := make(map[string]bool)
	checkedNotifies := make(map[string]bool)
	checkedServerNotifs := make(map[string]bool)
	checkedServerReqs := make(map[string]bool)

	notificationHandlers := map[string]bool{
		"handleNotification":     true,
		"runHeadless":            true,
		"observeHeadlessCapture": true,
	}
	serverRequestHandlers := map[string]bool{
		"handleServerRequest":      true,
		"decodeDirectApprovalView": true,
	}

	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		file, err := parser.ParseFile(fset, name, nil, 0)
		if err != nil {
			t.Fatalf("parse %s: %v", name, err)
		}

		for _, decl := range file.Decls {
			fn, ok := decl.(*ast.FuncDecl)
			if !ok || fn.Body == nil {
				continue
			}
			fnName := fn.Name.Name

			ast.Inspect(fn.Body, func(n ast.Node) bool {
				switch node := n.(type) {
				case *ast.CallExpr:
					sel, ok := node.Fun.(*ast.SelectorExpr)
					if !ok {
						return true
					}
					switch sel.Sel.Name {
					case "Call":
						if len(node.Args) < 2 {
							return true
						}
						methods := resolveStringExprInFunc(fn, node.Args[1])
						if len(methods) == 0 {
							t.Errorf("%s:%d (%s): unable to resolve RPC method argument to Call", name, fset.Position(node.Pos()).Line, fnName)
							return true
						}
						for _, method := range methods {
							checkedCalls[method] = true
							if !schemas.clientRequests[method] {
								t.Errorf("%s:%d calls unknown ClientRequest method %q", name, fset.Position(node.Pos()).Line, method)
							}
						}
					case "Send":
						// Headless app-server requests use requests.Send(method, params, ...).
						if ident, ok := sel.X.(*ast.Ident); !ok || ident.Name != "requests" || len(node.Args) < 1 {
							return true
						}
						methods := resolveStringExprInFunc(fn, node.Args[0])
						if len(methods) == 0 {
							t.Errorf("%s:%d (%s): unable to resolve RPC method argument to requests.Send", name, fset.Position(node.Pos()).Line, fnName)
							return true
						}
						for _, method := range methods {
							checkedCalls[method] = true
							if !schemas.clientRequests[method] {
								t.Errorf("%s:%d sends unknown ClientRequest method %q", name, fset.Position(node.Pos()).Line, method)
							}
						}
					case "Notify":
						if ident, ok := sel.X.(*ast.Ident); ok && ident.Name == "signal" {
							return true
						}
						if len(node.Args) < 2 {
							return true
						}
						methods := resolveStringExprInFunc(fn, node.Args[1])
						if len(methods) == 0 {
							t.Errorf("%s:%d (%s): unable to resolve RPC method argument to Notify", name, fset.Position(node.Pos()).Line, fnName)
							return true
						}
						for _, method := range methods {
							checkedNotifies[method] = true
							if !schemas.clientNotifications[method] {
								t.Errorf("%s:%d sends unknown ClientNotification method %q", name, fset.Position(node.Pos()).Line, method)
							}
						}
					}
				case *ast.SwitchStmt:
					isMethodSwitch := false
					switch tag := node.Tag.(type) {
					case *ast.SelectorExpr:
						isMethodSwitch = tag.Sel.Name == "Method"
					case *ast.Ident:
						isMethodSwitch = tag.Name == "method"
					}
					if !isMethodSwitch {
						return true
					}
					isNotif := notificationHandlers[fnName]
					isReq := serverRequestHandlers[fnName]
					if !isNotif && !isReq {
						return true
					}
					for _, stmt := range node.Body.List {
						clause, ok := stmt.(*ast.CaseClause)
						if !ok {
							continue
						}
						for _, expr := range clause.List {
							method, ok := unquoteStringLiteral(expr)
							if !ok {
								continue
							}
							if isNotif {
								checkedServerNotifs[method] = true
								if !schemas.serverNotifications[method] {
									t.Errorf("%s:%d (%s) handles unknown ServerNotification method %q", name, fset.Position(expr.Pos()).Line, fnName, method)
								}
							}
							if isReq {
								checkedServerReqs[method] = true
								if !schemas.serverRequests[method] {
									t.Errorf("%s:%d (%s) handles unknown ServerRequest method %q", name, fset.Position(expr.Pos()).Line, fnName, method)
								}
							}
						}
					}
				}
				return true
			})
		}
	}

	if len(checkedCalls) == 0 {
		t.Fatal("expected to verify at least one ClientRequest call")
	}
	if len(checkedNotifies) == 0 {
		t.Fatal("expected to verify at least one ClientNotification call")
	}
	if len(checkedServerNotifs) == 0 {
		t.Fatal("expected to verify at least one ServerNotification handler")
	}
	if len(checkedServerReqs) == 0 {
		t.Fatal("expected to verify at least one ServerRequest handler")
	}

	sortedCalls := make([]string, 0, len(checkedCalls))
	for m := range checkedCalls {
		sortedCalls = append(sortedCalls, m)
	}
	slices.Sort(sortedCalls)
	t.Logf("verified %d ClientRequest, %d ClientNotification, %d ServerNotification, and %d ServerRequest methods against Rust schemas",
		len(sortedCalls), len(checkedNotifies), len(checkedServerNotifs), len(checkedServerReqs))
}

func launchJSONFieldNames(typ reflect.Type) []string {
	var names []string
	for field := range typ.Fields() {
		if !field.IsExported() {
			continue
		}
		tag := field.Tag.Get("json")
		if tag == "-" {
			continue
		}
		name, _, _ := strings.Cut(tag, ",")
		if name == "" {
			if field.Anonymous {
				t := field.Type
				if t.Kind() == reflect.Pointer {
					t = t.Elem()
				}
				if t.Kind() == reflect.Struct {
					names = append(names, launchJSONFieldNames(t)...)
					continue
				}
			}
			name = field.Name
		}
		names = append(names, name)
	}
	return names
}

func TestLaunchWireStructsMatchRustJSONSchema(t *testing.T) {
	schemas := loadLaunchProtocolSchemas(t)

	cases := []struct {
		name    string
		typ     reflect.Type
		schemas []string
	}{
		{name: "directThreadStartResponse", typ: reflect.TypeFor[directThreadStartResponse](), schemas: []string{"ThreadStartResponse", "ThreadResumeResponse", "ThreadForkResponse"}},
		{name: "directSandboxPolicy", typ: reflect.TypeFor[directSandboxPolicy](), schemas: []string{"SandboxPolicy"}},
		{name: "directWireThread", typ: reflect.TypeFor[directWireThread](), schemas: []string{"Thread"}},
		{name: "directWireTurn", typ: reflect.TypeFor[directWireTurn](), schemas: []string{"Turn"}},
		{name: "directWireUsage", typ: reflect.TypeFor[directWireUsage](), schemas: []string{"ThreadTokenUsage"}},
		{name: "directChangeOrigin", typ: reflect.TypeFor[directChangeOrigin](), schemas: []string{"ThreadChangeOrigin"}},
		{name: "directThreadChangesResponse", typ: reflect.TypeFor[directThreadChangesResponse](), schemas: []string{"ThreadChangesReadResponse"}},
		{name: "directChangeFile", typ: reflect.TypeFor[directChangeFile](), schemas: []string{"ThreadChangeFileDiff"}},
		{name: "directWireModel", typ: reflect.TypeFor[directWireModel](), schemas: []string{"Model"}},
		{name: "directRateLimitWindow", typ: reflect.TypeFor[directRateLimitWindow](), schemas: []string{"RateLimitWindow"}},
		{name: "directRateLimitSnapshot", typ: reflect.TypeFor[directRateLimitSnapshot](), schemas: []string{"RateLimitSnapshot"}},
		{name: "directRateLimitsResponse", typ: reflect.TypeFor[directRateLimitsResponse](), schemas: []string{"GetAccountRateLimitsResponse"}},
		{name: "directResumePage", typ: reflect.TypeFor[directResumePage](), schemas: []string{"ThreadListResponse"}},
		{name: "directQueueSubmission", typ: reflect.TypeFor[directQueueSubmission](), schemas: []string{"QueuedSubmission"}},
		{name: "directRewindTarget", typ: reflect.TypeFor[directRewindTarget](), schemas: []string{"ThreadRevertParams"}},
		{name: "directRewindPreview", typ: reflect.TypeFor[directRewindPreview](), schemas: []string{"ThreadRevertPreviewResponse"}},
		{name: "directPermissionProfile", typ: reflect.TypeFor[directPermissionProfile](), schemas: []string{"ActivePermissionProfile"}},
		{name: "nativeFileChange", typ: reflect.TypeFor[nativeFileChange](), schemas: []string{"FileUpdateChange"}},
		{name: "nativePatchKind", typ: reflect.TypeFor[nativePatchKind](), schemas: []string{"PatchChangeKind"}},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			allowed := schemas.propertiesOf(t, tc.schemas...)
			fields := launchJSONFieldNames(tc.typ)
			if len(fields) == 0 {
				t.Fatalf("%s has no JSON fields", tc.name)
			}
			for _, field := range fields {
				if !allowed[field] {
					t.Errorf("%s JSON field %q not found in Rust schema %v", tc.name, field, tc.schemas)
				}
			}
		})
	}
}
