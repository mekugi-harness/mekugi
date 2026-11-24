package capture

import (
	"maps"
	"slices"
)

type requestFingerprint struct {
	Scope       string            `json:"scope"`
	Fields      map[string]string `json:"fields"`
	InputKind   string            `json:"input_kind"`
	Incremental bool              `json:"incremental,omitempty"`
	Items       []string          `json:"items"`
	ItemCount   int               `json:"item_count"`
	Complete    bool              `json:"complete"`
	RequestKey  string            `json:"request_key,omitempty"`
	RoutingKey  string            `json:"routing_key,omitempty"`
	// Nil means unobserved (older evidence or the body-only native seam).
	// An observed empty value means no nonempty turn-state header was sent.
	TurnState *string `json:"turn_state,omitempty"`
}

type prefixComparison struct {
	Status        string   `json:"status"`
	CommonItems   int      `json:"common_items"`
	ChangedFields []string `json:"changed_fields"`
}

type cacheDiagnosis struct {
	PreviousSequence    uint64           `json:"previous_sequence"`
	Client              prefixComparison `json:"client"`
	Projected           prefixComparison `json:"projected"`
	Provider            prefixComparison `json:"provider"`
	Routing             string           `json:"routing"`
	RequestKey          string           `json:"request_key"`
	TurnStateForwarding string           `json:"turn_state_forwarding,omitempty"`
}

func cloneFingerprint(source *requestFingerprint) *requestFingerprint {
	if source == nil {
		return nil
	}
	result := *source
	result.Fields = maps.Clone(source.Fields)
	result.Items = slices.Clone(source.Items)
	if source.TurnState != nil {
		value := *source.TurnState
		result.TurnState = &value
	}
	return &result
}
