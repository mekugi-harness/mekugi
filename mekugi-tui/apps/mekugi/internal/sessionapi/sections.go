package sessionapi

type DeferredSections struct {
	Session *ContentReference `json:"session,omitzero"`
	Input   *ContentReference `json:"input,omitzero"`
	Prompts *ContentReference `json:"prompts,omitzero"`
	Status  *ContentReference `json:"status,omitzero"`
	Review  *ContentReference `json:"review,omitzero"`
	Live    *ContentReference `json:"live,omitzero"`
}

// encodeShared leaves RPC envelope room after two feed windows and a roster
// window. Auxiliary content has explicit references instead of silent loss.
