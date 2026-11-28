package binding_test

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

type schemaDoc struct {
	Ref         string                    `json:"$ref"`
	Properties  map[string]jsontext.Value `json:"properties"`
	Definitions map[string]jsontext.Value `json:"definitions"`
	AllOf       []jsontext.Value          `json:"allOf"`
	AnyOf       []jsontext.Value          `json:"anyOf"`
	OneOf       []jsontext.Value          `json:"oneOf"`
}

type precomputedExports struct {
	JSONSchema map[string]string `json:"json_schema"`
}

type schemaRegistry struct {
	defs map[string][]schemaDoc
}

func loadSchemaRegistry(t *testing.T) *schemaRegistry {
	t.Helper()
	reg := &schemaRegistry{defs: make(map[string][]schemaDoc)}

	addBundle := func(name string, raw []byte) {
		var doc schemaDoc
		if err := json.Unmarshal(raw, &doc); err != nil {
			t.Fatalf("parse %s: %v", name, err)
		}
		for k, rawDef := range doc.Definitions {
			if len(rawDef) == 0 || rawDef[0] != '{' {
				continue
			}
			var def schemaDoc
			if err := json.Unmarshal(rawDef, &def); err != nil {
				t.Fatalf("parse %s definition %s: %v", name, k, err)
			}
			reg.defs[k] = append(reg.defs[k], def)
		}
	}

	stableDir := filepath.Join("testdata", "app-server-protocol-json")
	for _, filename := range []string{
		"codex_app_server_protocol.schemas.json",
		"codex_app_server_protocol.v2.schemas.json",
	} {
		raw, err := os.ReadFile(filepath.Join(stableDir, filename))
		if err != nil {
			t.Fatalf("read stable %s: %v", filename, err)
		}
		addBundle("stable/"+filename, raw)
	}

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
	var exp precomputedExports
	if err := json.Unmarshal(expRaw, &exp); err != nil {
		t.Fatalf("parse experimental exports: %v", err)
	}
	for _, filename := range []string{
		"codex_app_server_protocol.schemas.json",
		"codex_app_server_protocol.v2.schemas.json",
	} {
		raw, ok := exp.JSONSchema[filename]
		if !ok {
			t.Fatalf("experimental exports missing %s", filename)
		}
		addBundle("experimental/"+filename, []byte(raw))
	}

	return reg
}

func (r *schemaRegistry) propertiesOf(t *testing.T, names ...string) map[string]bool {
	t.Helper()
	props := make(map[string]bool)
	visited := make(map[string]bool)

	var collect func(doc schemaDoc)
	collectRaw := func(raw jsontext.Value) {
		if len(raw) == 0 || raw[0] != '{' {
			return
		}
		var sub schemaDoc
		if err := json.Unmarshal(raw, &sub); err != nil {
			t.Fatalf("parse subschema: %v", err)
		}
		collect(sub)
	}
	collect = func(doc schemaDoc) {
		if doc.Ref != "" {
			if refName, ok := strings.CutPrefix(doc.Ref, "#/definitions/"); ok && !visited[refName] {
				visited[refName] = true
				for _, target := range r.defs[refName] {
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
		docs := r.defs[name]
		if len(docs) == 0 {
			t.Fatalf("schema definition %q not found in Rust-generated JSON schemas", name)
		}
		for _, doc := range docs {
			collect(doc)
		}
	}
	return props
}

func jsonFieldNames(typ reflect.Type) []string {
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
					names = append(names, jsonFieldNames(t)...)
					continue
				}
			}
			name = field.Name
		}
		names = append(names, name)
	}
	return names
}

func TestBindingWireStructsMatchRustJSONSchema(t *testing.T) {
	reg := loadSchemaRegistry(t)

	cases := []struct {
		name    string
		typ     reflect.Type
		schemas []string
	}{
		{name: "TokenUsage", typ: reflect.TypeFor[binding.TokenUsage](), schemas: []string{"TokenUsageBreakdown"}},
		{name: "Turn", typ: reflect.TypeFor[binding.Turn](), schemas: []string{"Turn"}},
		{name: "TurnError", typ: reflect.TypeFor[binding.TurnError](), schemas: []string{"TurnError"}},
		{name: "HistoryTurn", typ: reflect.TypeFor[binding.HistoryTurn](), schemas: []string{"Turn"}},
		{name: "ThreadInfo", typ: reflect.TypeFor[binding.ThreadInfo](), schemas: []string{"Thread"}},
		{name: "ThreadItem", typ: reflect.TypeFor[binding.ThreadItem](), schemas: []string{"ThreadItem"}},
		{name: "FileChange", typ: reflect.TypeFor[binding.FileChange](), schemas: []string{"FileUpdateChange"}},
		{name: "CommandAction", typ: reflect.TypeFor[binding.CommandAction](), schemas: []string{"CommandAction"}},
		{name: "WebSearchAction", typ: reflect.TypeFor[binding.WebSearchAction](), schemas: []string{"WebSearchAction"}},
		{name: "InputByteRange", typ: reflect.TypeFor[binding.InputByteRange](), schemas: []string{"ByteRange"}},
		{name: "InputTextElement", typ: reflect.TypeFor[binding.InputTextElement](), schemas: []string{"TextElement"}},
		{name: "QuestionRequest", typ: reflect.TypeFor[binding.QuestionRequest](), schemas: []string{"ToolRequestUserInputParams"}},
		{name: "ResolvedRequest", typ: reflect.TypeFor[binding.ResolvedRequest](), schemas: []string{"ServerRequestResolvedNotification"}},
		{name: "ShellGuardApprovalParams", typ: reflect.TypeFor[binding.ShellGuardApprovalParams](), schemas: []string{"ShellGuardRequestApprovalParams"}},
		{name: "ShellGuardApprovalResponse", typ: reflect.TypeFor[binding.ShellGuardApprovalResponse](), schemas: []string{"ShellGuardRequestApprovalResponse"}},
		{name: "ShellGuardSecretParams", typ: reflect.TypeFor[binding.ShellGuardSecretParams](), schemas: []string{"ShellGuardRequestSecretParams"}},
		{name: "ShellGuardSecretResponse", typ: reflect.TypeFor[binding.ShellGuardSecretResponse](), schemas: []string{"ShellGuardRequestSecretResponse"}},
		{name: "SkillCatalog", typ: reflect.TypeFor[binding.SkillCatalog](), schemas: []string{"SkillsListResponse"}},
		{
			name:    "ApprovalRequest",
			typ:     reflect.TypeFor[binding.ApprovalRequest](),
			schemas: []string{"CommandExecutionRequestApprovalParams", "FileChangeRequestApprovalParams", "PermissionsRequestApprovalParams"},
		},
		{
			name: "Notification",
			typ:  reflect.TypeFor[binding.Notification](),
			schemas: []string{
				"ErrorNotification",
				"ThreadStartedNotification",
				"ThreadStatusChangedNotification",
				"TurnStartedNotification",
				"TurnCompletedNotification",
				"ItemStartedNotification",
				"ItemCompletedNotification",
				"AgentMessageDeltaNotification",
				"CommandExecutionOutputDeltaNotification",
				"TerminalInteractionNotification",
				"HookStartedNotification",
				"HookCompletedNotification",
				"ThreadTokenUsageUpdatedNotification",
			},
		},
		{name: "appserver.HookRun", typ: reflect.TypeFor[appserver.HookRun](), schemas: []string{"HookRunSummary"}},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			allowed := reg.propertiesOf(t, tc.schemas...)
			fields := jsonFieldNames(tc.typ)
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
