package binding

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

func (r *schemaRegistry) propertySchemasOfDocs(t *testing.T, docs []schemaDoc) map[string][]schemaDoc {
	t.Helper()
	props := make(map[string][]schemaDoc)
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
		for k, rawProp := range doc.Properties {
			if len(rawProp) > 0 && rawProp[0] == '{' {
				var sub schemaDoc
				if err := json.Unmarshal(rawProp, &sub); err != nil {
					t.Fatalf("parse property %q subschema: %v", k, err)
				}
				props[k] = append(props[k], sub)
			} else {
				props[k] = append(props[k], schemaDoc{})
			}
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

	for _, doc := range docs {
		collect(doc)
	}
	return props
}

func (r *schemaRegistry) docsForNames(t *testing.T, names ...string) []schemaDoc {
	t.Helper()
	var docs []schemaDoc
	for _, name := range names {
		found := r.defs[name]
		if len(found) == 0 {
			t.Fatalf("schema definition %q not found in Rust-generated JSON schemas", name)
		}
		docs = append(docs, found...)
	}
	return docs
}

type jsonFieldSpec struct {
	name string
	typ  reflect.Type
}

func jsonFields(typ reflect.Type) []jsonFieldSpec {
	var fields []jsonFieldSpec
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
					fields = append(fields, jsonFields(t)...)
					continue
				}
			}
			name = field.Name
		}
		fields = append(fields, jsonFieldSpec{name: name, typ: field.Type})
	}
	return fields
}

func (r *schemaRegistry) verifyStruct(t *testing.T, path string, typ reflect.Type, docs []schemaDoc, recursive bool) {
	t.Helper()
	allowed := r.propertySchemasOfDocs(t, docs)
	fields := jsonFields(typ)
	if len(fields) == 0 {
		t.Fatalf("%s has no JSON fields", path)
	}
	for _, field := range fields {
		propDocs, ok := allowed[field.name]
		if !ok {
			t.Errorf("%s JSON field %q not found in Rust schema", path, field.name)
			continue
		}
		if !recursive {
			continue
		}
		ft := field.typ
		for ft.Kind() == reflect.Pointer {
			ft = ft.Elem()
		}
		if ft.Kind() == reflect.Struct && ft != reflect.TypeFor[jsontext.Value]() {
			r.verifyStruct(t, path+"."+field.name, ft, propDocs, true)
		}
	}
}

func TestBindingWireStructsMatchRustJSONSchema(t *testing.T) {
	reg := loadSchemaRegistry(t)

	cases := []struct {
		name      string
		typ       reflect.Type
		schemas   []string
		recursive bool
	}{
		{name: "TokenUsage", typ: reflect.TypeFor[TokenUsage](), schemas: []string{"TokenUsageBreakdown"}},
		{name: "Turn", typ: reflect.TypeFor[Turn](), schemas: []string{"Turn"}},
		{name: "TurnError", typ: reflect.TypeFor[TurnError](), schemas: []string{"TurnError"}},
		{name: "HistoryTurn", typ: reflect.TypeFor[HistoryTurn](), schemas: []string{"Turn"}},
		{name: "ThreadInfo", typ: reflect.TypeFor[ThreadInfo](), schemas: []string{"Thread"}},
		{name: "threadSourceWire", typ: reflect.TypeFor[threadSourceWire](), schemas: []string{"SessionSource"}, recursive: true},
		{name: "ThreadItem", typ: reflect.TypeFor[ThreadItem](), schemas: []string{"ThreadItem"}},
		{name: "FileChange", typ: reflect.TypeFor[FileChange](), schemas: []string{"FileUpdateChange"}},
		{name: "CommandAction", typ: reflect.TypeFor[CommandAction](), schemas: []string{"CommandAction"}},
		{name: "WebSearchAction", typ: reflect.TypeFor[WebSearchAction](), schemas: []string{"WebSearchAction"}},
		{name: "InputByteRange", typ: reflect.TypeFor[InputByteRange](), schemas: []string{"ByteRange"}},
		{name: "InputTextElement", typ: reflect.TypeFor[InputTextElement](), schemas: []string{"TextElement"}},
		{name: "QuestionRequest", typ: reflect.TypeFor[QuestionRequest](), schemas: []string{"ToolRequestUserInputParams"}},
		{name: "ResolvedRequest", typ: reflect.TypeFor[ResolvedRequest](), schemas: []string{"ServerRequestResolvedNotification"}},
		{name: "ShellGuardApprovalParams", typ: reflect.TypeFor[ShellGuardApprovalParams](), schemas: []string{"ShellGuardRequestApprovalParams"}},
		{name: "ShellGuardApprovalResponse", typ: reflect.TypeFor[ShellGuardApprovalResponse](), schemas: []string{"ShellGuardRequestApprovalResponse"}},
		{name: "ShellGuardSecretParams", typ: reflect.TypeFor[ShellGuardSecretParams](), schemas: []string{"ShellGuardRequestSecretParams"}},
		{name: "ShellGuardSecretResponse", typ: reflect.TypeFor[ShellGuardSecretResponse](), schemas: []string{"ShellGuardRequestSecretResponse"}},
		{name: "SkillCatalog", typ: reflect.TypeFor[SkillCatalog](), schemas: []string{"SkillsListResponse"}},
		{
			name:    "ApprovalRequest",
			typ:     reflect.TypeFor[ApprovalRequest](),
			schemas: []string{"CommandExecutionRequestApprovalParams", "FileChangeRequestApprovalParams", "PermissionsRequestApprovalParams"},
		},
		{
			name: "Notification",
			typ:  reflect.TypeFor[Notification](),
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
			docs := reg.docsForNames(t, tc.schemas...)
			reg.verifyStruct(t, tc.name, tc.typ, docs, tc.recursive)
		})
	}
}
