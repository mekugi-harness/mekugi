package capture

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
)

type usageMetrics struct {
	CachedObserved      bool                       `json:"cached_observed,omitzero"`
	ReasoningObserved   bool                       `json:"reasoning_observed,omitzero"`
	OutputThroughput    inference.OutputThroughput `json:"output_throughput,omitzero"`
	CompleteAttempts    uint64                     `json:"complete_attempts"`
	IncompleteAttempts  uint64                     `json:"incomplete_attempts"`
	MissingAttempts     uint64                     `json:"missing_attempts"`
	InputTokens         uint64                     `json:"input_tokens"`
	CachedInputTokens   uint64                     `json:"cached_input_tokens"`
	UncachedInputTokens uint64                     `json:"uncached_input_tokens"`
	OutputTokens        uint64                     `json:"output_tokens"`
	ReasoningTokens     uint64                     `json:"reasoning_tokens"`
	ProviderAttempts    uint64                     `json:"provider_attempts"`
}

type requestTotals struct {
	Retries          uint64 `json:"retries"`
	Logical          uint64 `json:"logical"`
	ProviderAttempts uint64 `json:"provider_attempts"`
	Completed        uint64 `json:"completed"`
	Failed           uint64 `json:"failed"`
}

type cacheMetrics struct {
	ProviderCacheRate *float64 `json:"provider_cache_rate"`
}

type payloadTotals struct {
	Bytes uint64 `json:"bytes"`
}

type transportMetrics struct {
	ClientControlRequests    payloadTotals `json:"client_control_requests,omitzero"`
	ClientRequests           payloadTotals `json:"client_requests,omitzero"`
	ProviderAttemptRequests  payloadTotals `json:"provider_attempt_requests,omitzero"`
	ProviderControlRequests  payloadTotals `json:"provider_control_requests,omitzero"`
	ProviderControlResponses payloadTotals `json:"provider_control_responses,omitzero"`
	ProviderResponses        payloadTotals `json:"provider_responses,omitzero"`
	ClientControlResponses   payloadTotals `json:"client_control_responses,omitzero"`
	ClientResponses          payloadTotals `json:"client_responses,omitzero"`
}

type captureHealth struct {
	Records                uint64 `json:"records"`
	CaptureErrors          uint64 `json:"capture_errors"`
	Incomplete             uint64 `json:"incomplete_records"`
	MissingProvider        uint64 `json:"missing_provider_records"`
	AttemptGaps            uint64 `json:"provider_attempt_gaps"`
	WriteErrors            uint64 `json:"write_errors"`
	SkippedRequests        uint64 `json:"skipped_requests"`
	DroppedExchangeDetails uint64 `json:"dropped_exchange_details"`
}

type providerAttemptMetrics struct {
	DurationMillis       uint64                    `json:"duration_ms"`
	StatusCode           int                       `json:"status_code"`
	CaptureError         string                    `json:"capture_error,omitempty"`
	Transport            string                    `json:"transport,omitempty"`
	ProviderResponse     *providerResponseEvidence `json:"provider_response,omitempty"`
	Attempt              uint64                    `json:"attempt"`
	Model                string                    `json:"model,omitempty"`
	Status               string                    `json:"status"`
	ResponseComplete     bool                      `json:"response_complete"`
	Usage                *usageMetrics             `json:"usage,omitempty"`
	Request              payloadMetrics            `json:"request"`
	Fingerprint          *requestFingerprint       `json:"cache_fingerprint,omitempty"`
	ProjectedFingerprint *requestFingerprint       `json:"projected_fingerprint,omitempty"`
	Response             payloadMetrics            `json:"response"`
}

type exchangeMetrics struct {
	StatusCode          int                      `json:"status_code"`
	ResponseComplete    bool                     `json:"response_complete"`
	CaptureError        string                   `json:"capture_error,omitempty"`
	DurationMillis      uint64                   `json:"duration_ms"`
	PredecessorSequence uint64                   `json:"predecessor_sequence,omitempty"`
	RequestKind         string                   `json:"request_kind,omitempty"`
	ClientFingerprint   *requestFingerprint      `json:"client_fingerprint,omitempty"`
	CacheDiagnosis      *cacheDiagnosis          `json:"cache_diagnostics,omitempty"`
	Sequence            uint64                   `json:"sequence"`
	ThreadID            string                   `json:"thread_id,omitempty"`
	Model               string                   `json:"model,omitempty"`
	ProviderAttempts    []providerAttemptMetrics `json:"provider_attempts"`
	Status              string                   `json:"status"`
	Usage               *usageMetrics            `json:"usage,omitempty"`
	ClientRequest       payloadMetrics           `json:"client_request,omitzero"`
	ClientResponse      payloadMetrics           `json:"client_response,omitzero"`
}

type MetricsSnapshot struct {
	StorageWrites *persistence.Snapshot `json:"storage_writes,omitempty"`
	Schema        string                `json:"schema"`
	Mode          string                `json:"mode"`
	Requests      requestTotals         `json:"requests"`
	Usage         usageMetrics          `json:"usage"`
	Cache         cacheMetrics          `json:"cache"`
	Transport     transportMetrics      `json:"transport"`
	Exchanges     []exchangeMetrics     `json:"exchanges"`
	Capture       captureHealth         `json:"capture"`
}

func (source MetricsSnapshot) Clone() MetricsSnapshot {
	clone := source
	if source.StorageWrites != nil {
		clone.StorageWrites = new(*source.StorageWrites)
	}
	if source.Cache.ProviderCacheRate != nil {
		rate := *source.Cache.ProviderCacheRate
		clone.Cache.ProviderCacheRate = &rate
	}
	clone.Exchanges = slices.Clone(source.Exchanges)
	for index, exchange := range source.Exchanges {
		clone.Exchanges[index] = exchange
		if exchange.CacheDiagnosis != nil {
			diagnosis := *exchange.CacheDiagnosis
			diagnosis.Client.ChangedFields = slices.Clone(diagnosis.Client.ChangedFields)
			diagnosis.Projected.ChangedFields = slices.Clone(diagnosis.Projected.ChangedFields)
			diagnosis.Provider.ChangedFields = slices.Clone(diagnosis.Provider.ChangedFields)
			clone.Exchanges[index].CacheDiagnosis = &diagnosis
		}
		clone.Exchanges[index].ClientFingerprint = cloneFingerprint(exchange.ClientFingerprint)
		clone.Exchanges[index].ProviderAttempts = slices.Clone(exchange.ProviderAttempts)
		for attemptIndex, attempt := range exchange.ProviderAttempts {
			clone.Exchanges[index].ProviderAttempts[attemptIndex] = attempt
			clone.Exchanges[index].ProviderAttempts[attemptIndex].ProviderResponse = cloneProviderEvidence(attempt.ProviderResponse)
			clone.Exchanges[index].ProviderAttempts[attemptIndex].Fingerprint = cloneFingerprint(attempt.Fingerprint)
			clone.Exchanges[index].ProviderAttempts[attemptIndex].ProjectedFingerprint = cloneFingerprint(attempt.ProjectedFingerprint)
			if attempt.Usage != nil {
				usage := *attempt.Usage
				clone.Exchanges[index].ProviderAttempts[attemptIndex].Usage = &usage
			}
		}
		if exchange.Usage != nil {
			usage := *exchange.Usage
			clone.Exchanges[index].Usage = &usage
		}
	}
	return clone
}
