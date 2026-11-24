package executionobservation

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"strings"

	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

// ObservePreparedContent receives the final semantic input before transport
// history compression. Attachments are retained with their submitted user input.
func (t *mekugiResponseTransform) ObservePreparedContent(request *codexresponses.Request) {
	var items []map[string]jsontext.Value
	if t.owners.Audience == nil || t.threadID == "" || json.Unmarshal(request.Fields["input"], &items) != nil {
		return
	}
	sources := make(map[string]string)
	originalOutputs := make(map[string]string)
	var originals []map[string]jsontext.Value
	_ = json.Unmarshal(request.OriginalField("input"), &originals)
	for _, item := range originals {
		kind := responses.StringField(item, "type")
		if kind == "custom_tool_call_output" || kind == "function_call_output" {
			if text, ok := preparedOutputText(item["output"]); ok {
				originalOutputs[kind+"\x00"+responses.StringField(item, "call_id")] = text
			}
		}
		if responses.StringField(item, "type") == "custom_tool_call" && responses.StringField(item, "name") == t.codeModeToolName {
			sources[responses.StringField(item, "call_id")] = responses.StringField(item, "input")
		}
	}
	for _, item := range items {
		kind := responses.StringField(item, "type")
		if kind != "custom_tool_call_output" && kind != "function_call_output" {
			continue
		}
		id := responses.StringField(item, "call_id")
		text, readable := preparedOutputText(item["output"])
		original, known := originalOutputs[kind+"\x00"+id]
		content := activity.PreparedContent{ID: id, Text: text, Altered: readable && known && text != original}
		if source, exists := sources[id]; exists {
			content.Cell = true
			if history, ok := t.visible[id]; ok {
				if history.CarrierPayload != "" {
					source = history.CarrierPayload
				} else if history.Script != "" {
					source = history.Script
				}
			}
			content.Calls = t.owners.NativeTrace.ReadCell(t.threadID, id, source).ToolCalls()
		}
		t.owners.Audience.ObservePrepared(t.threadID, content)
	}
}

// preparedOutputText removes only recognized transport text wrappers. JSON printed
// by a tool is literal text; mixed media and extended objects retain their facts.
func preparedOutputText(raw jsontext.Value) (string, bool) {
	if len(raw) == 0 {
		return "", false
	}
	if raw.Kind() == '"' {
		var text string
		err := json.Unmarshal(raw, &text)
		return text, err == nil
	}
	if raw.Kind() == '[' {
		var parts []jsontext.Value
		if json.Unmarshal(raw, &parts) != nil {
			return "", false
		}
		texts := make([]string, 0, len(parts))
		hasText := false
		for _, part := range parts {
			var fields map[string]jsontext.Value
			var text string
			if json.Unmarshal(part, &fields) == nil && len(fields) == 2 &&
				responses.StringField(fields, "type") == "input_text" && json.Unmarshal(fields["text"], &text) == nil {
				hasText = true
				texts = append(texts, text)
			} else {
				texts = append(texts, string(part))
			}
		}
		if !hasText {
			return string(raw), true
		}
		return strings.Join(texts, "\n"), true
	}
	return string(raw), true
}
