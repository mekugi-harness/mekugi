package capture

type providerResponseEvidence struct {
	RequestID         string  `json:"request_id,omitempty"`
	HeaderModel       string  `json:"header_model,omitempty"`
	Model             string  `json:"model,omitempty"`
	ServiceTier       string  `json:"service_tier,omitempty"`
	CachedTokensState string  `json:"cached_tokens_state"`
	CachedTokens      *uint64 `json:"cached_tokens,omitempty"`
}

func cloneProviderEvidence(source *providerResponseEvidence) *providerResponseEvidence {
	if source == nil {
		return nil
	}
	result := *source
	if source.CachedTokens != nil {
		count := *source.CachedTokens
		result.CachedTokens = &count
	}
	return &result
}
