package sessionpresentation

import (
	"cmp"
	"fmt"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/pathdisplay"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
	usagepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/presentation"
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

type cachePrefixComparison struct {
	Status        string   `json:"status"`
	CommonItems   int      `json:"common_items"`
	ChangedFields []string `json:"changed_fields,omitempty"`
}

type cacheDiagnosis struct {
	PreviousSequence    uint64                `json:"previous_sequence"`
	Client              cachePrefixComparison `json:"client"`
	Projected           cachePrefixComparison `json:"projected"`
	Provider            cachePrefixComparison `json:"provider"`
	Routing             string                `json:"routing,omitempty"`
	RequestKey          string                `json:"request_key,omitempty"`
	TurnStateForwarding string                `json:"turn_state_forwarding,omitempty"`
}

type providerResponseEvidence struct {
	ServiceTier       string  `json:"service_tier,omitempty"`
	Model             string  `json:"model,omitempty"`
	HeaderModel       string  `json:"header_model,omitempty"`
	RequestID         string  `json:"request_id,omitempty"`
	CachedTokensState string  `json:"cached_tokens_state,omitempty"`
	CachedTokens      *uint64 `json:"cached_tokens,omitempty"`
}

type providerAttemptMetrics struct {
	DurationMillis   uint64                    `json:"duration_ms"`
	StatusCode       int                       `json:"status_code"`
	CaptureError     string                    `json:"capture_error,omitempty"`
	Transport        string                    `json:"transport,omitempty"`
	ProviderResponse *providerResponseEvidence `json:"provider_response,omitempty"`
	Attempt          uint64                    `json:"attempt"`
	Model            string                    `json:"model,omitempty"`
	Status           string                    `json:"status"`
	ResponseComplete bool                      `json:"response_complete"`
	Usage            *usageMetrics             `json:"usage,omitempty"`
	Request          payloadTotals             `json:"request"`
	Response         payloadTotals             `json:"response"`
}

type exchangeMetrics struct {
	StatusCode       int                      `json:"status_code"`
	ResponseComplete bool                     `json:"response_complete"`
	CaptureError     string                   `json:"capture_error,omitempty"`
	DurationMillis   uint64                   `json:"duration_ms"`
	RequestKind      string                   `json:"request_kind,omitempty"`
	CacheDiagnosis   *cacheDiagnosis          `json:"cache_diagnostics,omitempty"`
	Sequence         uint64                   `json:"sequence"`
	ThreadID         string                   `json:"thread_id,omitempty"`
	Model            string                   `json:"model,omitempty"`
	ProviderAttempts []providerAttemptMetrics `json:"provider_attempts"`
	Status           string                   `json:"status"`
	Usage            *usageMetrics            `json:"usage,omitempty"`
	ClientRequest    payloadTotals            `json:"client_request,omitzero"`
	ClientResponse   payloadTotals            `json:"client_response,omitzero"`
}

// MetricsSnapshot carries optional session metrics for /session panel rendering.
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

// MetricViews names the /session views in tab order.
var MetricViews = []string{"Overview", "Transport", "Exchanges", "Features"}

// MetricFeatures identifies the page whose native counters are read on demand.
const MetricFeatures = 3

// FeatureCounts are model-facing feature uses and counted attempts in this launch.
type FeatureCounts struct {
	observation.FeatureUse
}

// FeatureMetrics separates the selected thread from the sum of all of its
// admitted descendants, including settled ones.
type FeatureMetrics struct {
	Self, Descendants FeatureCounts
	// Thread names Self for display; Agents counts the summed descendants.
	Thread string
	Agents int

	Unavailable bool
}

// featureFields compares Self with Descendants in one table per feature family.
// Each family's first row is its total; later rows count subsets or failed attempts.
func featureFields(f *FeatureMetrics) (fields []StatusField) {
	if f == nil {
		return []StatusField{{Group: "Features", Value: "Feature use is unavailable for this frontend. Live launch counts are not restored from saved sessions."}}
	}
	fields = append(fields, StatusField{Group: "Features", Value: fmt.Sprintf("Self is %s; Descendants sums its admitted agents (%d), including completed ones. Counts cover this launch and are not restored on resume.", cmp.Or(f.Thread, "the displayed thread"), f.Agents)})
	type row struct {
		label             string
		self, descendants any
	}
	table := func(group string, rows ...row) {
		fields = append(fields, StatusField{Group: group, Heading: true, Values: []string{"Self", "Descendants"}})
		for _, r := range rows {
			values := []string{fmt.Sprint(r.self), fmt.Sprint(r.descendants)}
			if f.Unavailable {
				values = []string{"unavailable", "unavailable"}
			}
			fields = append(fields, StatusField{Group: group, Label: r.label, Values: values})
		}
	}
	self, desc := f.Self, f.Descendants
	table("Compaction", row{"Total", self.Compactions, desc.Compactions}, row{"Self-initiated", self.SelfCompactions, desc.SelfCompactions}, row{"With preload", self.CompactionPreloads, desc.CompactionPreloads}, row{"Self completed", self.SelfCompactionsCompleted, desc.SelfCompactionsCompleted}, row{"Self failed", self.SelfCompactionFailures, desc.SelfCompactionFailures})
	table("Spawns", row{"Total", self.Spawns, desc.Spawns}, row{"With preload", self.SpawnPreloads, desc.SpawnPreloads}, row{"Preload rejected", self.PreloadRejections, desc.PreloadRejections})
	table("Delegate finals", row{"Completed", self.Finals, desc.Finals}, row{"With edit ranges", self.CompletionChanges, desc.CompletionChanges})
	table("Messages", row{"Sent", self.Messages, desc.Messages}, row{"With btw", self.SideQuestions, desc.SideQuestions}, row{"btw rejected", self.SideQuestionRejections, desc.SideQuestionRejections}, row{"btw failed", self.SideQuestionFailures, desc.SideQuestionFailures})
	table("Waits", row{"Registered", self.Waits, desc.Waits}, row{"All mode", self.AllWaits, desc.AllWaits}, row{"Progress enabled", self.ProgressWaits, desc.ProgressWaits}, row{"Progress delivered", self.ProgressYields, desc.ProgressYields})
	table("Source-bearing sends", row{"Total", self.AssignmentSources + self.MessageSources, desc.AssignmentSources + desc.MessageSources}, row{"Assignments", self.AssignmentSources, desc.AssignmentSources}, row{"Messages", self.MessageSources, desc.MessageSources})
	table("Output deduplication", row{"Saved bytes", self.OutputDeduplicationBytes, desc.OutputDeduplicationBytes}, row{"Saved tokens (est.)", self.OutputDeduplicationTokens, desc.OutputDeduplicationTokens})
	fields = append(fields, StatusField{Group: "Output deduplication", Value: "Net model-input savings after reference overhead, summed across requests. Tokens use a local 4-byte estimate, not provider billing savings."})
	return fields
}

// MetricFields projects the same session metrics for terminal and browser panels.
// Source: apps/mekugi/internal/terminalsession/panel_metrics.go@a90c7860.
func MetricFields(body *PanelView, tab, exchange int) (fields []StatusField) {
	add := func(group, label string, value any) {
		text := fmt.Sprint(value)
		if text == "" {
			return
		}
		fields = append(fields, StatusField{Group: group, Label: label, Value: text})
	}
	payload := func(group, label string, bytes uint64) {
		add(group, label, fmt.Sprintf("%d bytes", bytes))
	}
	usage := func(group string, input, cached, uncached, output, reasoning uint64) {
		for _, row := range []struct {
			label string
			value uint64
		}{{"Input", input}, {"Cached input", cached}, {"Uncached input", uncached}, {"Output", output}, {"Reasoning", reasoning}} {
			add(group, row.label, row.value)
		}
	}
	optional := func(group, label string, value *uint64) {
		if value == nil {
			add(group, label, "unavailable")
		} else {
			add(group, label, *value)
		}
	}
	if tab == MetricFeatures {
		fields = featureFields(body.Features)
		return
	}
	s := *body.Metrics
	native := s.Mode == "native"
	measuredUsage := func(group string, input, cached, uncached, output, reasoning uint64, seen, cachedSeen, reasoningSeen bool) {
		if !native {
			usage(group, input, cached, uncached, output, reasoning)
			return
		}
		for _, f := range []struct {
			label string
			value uint64
			known bool
		}{
			{"Input", input, seen}, {"Cached input", cached, cachedSeen}, {"Uncached input", uncached, cachedSeen},
			{"Output", output, seen}, {"Reasoning", reasoning, reasoningSeen},
		} {
			if f.known {
				add(group, f.label, f.value)
			} else {
				add(group, f.label, "unavailable")
			}
		}
	}
	if tab == 0 && body.DebugDirectory != "" {
		add("Debug", "Directory", pathdisplay.ForWorkspace(body.Workspace, body.DebugDirectory))
		fields[len(fields)-1].Path = true
	}
	if tab == 0 && s.StorageWrites != nil {
		add("Storage writes", "Application bytes", s.StorageWrites.Bytes)
		add("Storage writes", "", s.StorageWrites.Scope)
	}
	if s.Schema == "" {
		add("Capture", "", "Metrics are unavailable for this frontend. Live launch metrics are not restored from saved sessions.")
		return
	}
	if native {
		switch tab {
		case 0:
			add("Responses", "Scope", "All threads in this launch")
			add("Responses", "Completed", s.Requests.Completed)
			add("Responses", "Terminal errors", s.Requests.Failed)
			measuredUsage("Usage", s.Usage.InputTokens, s.Usage.CachedInputTokens, s.Usage.UncachedInputTokens, s.Usage.OutputTokens, s.Usage.ReasoningTokens,
				s.Usage.CompleteAttempts+s.Usage.IncompleteAttempts > 0, s.Usage.CachedObserved, s.Usage.ReasoningObserved)
			add("Usage evidence", "Partial", s.Usage.IncompleteAttempts)
			add("Usage evidence", "Missing", s.Usage.MissingAttempts)
			add("Capture", "Write errors", s.Capture.WriteErrors)
		case 1:
			add("Transport", "", "Native responses do not report transport measurements.")
		case 2:
			if len(s.Exchanges) == 0 {
				add("Responses", "", "No response completions or terminal errors observed.")
				break
			}
			index := max(0, min(exchange, len(s.Exchanges)-1))
			e := s.Exchanges[index]
			add("Response", "Retained", fmt.Sprintf("%d / %d", index+1, len(s.Exchanges)))
			add("Response", "Thread", e.ThreadID)
			add("Response", "Model", e.Model)
			add("Response", "Status", e.Status)
			if e.Usage == nil {
				add("Response", "Usage", "unavailable")
			} else {
				measuredUsage("Response", e.Usage.InputTokens, e.Usage.CachedInputTokens, e.Usage.UncachedInputTokens, e.Usage.OutputTokens, e.Usage.ReasoningTokens, true, e.Usage.CachedObserved, e.Usage.ReasoningObserved)
			}
		}
		return
	}
	switch tab {
	case 0:
		add("Requests", "Scope", "All threads in this launch · "+s.Mode)
		add("Requests", "Logical", s.Requests.Logical)
		add("Requests", "Attempts", s.Requests.ProviderAttempts)
		add("Requests", "Retries", s.Requests.Retries)
		add("Requests", "Completed", s.Requests.Completed)
		add("Requests", "Failed", s.Requests.Failed)
		add("Usage", "", "Provider-reported consumption, not local estimates. Missing normalized telemetry may default to zero; inspect Exchanges for explicit evidence.")
		measuredUsage("Usage", s.Usage.InputTokens, s.Usage.CachedInputTokens, s.Usage.UncachedInputTokens, s.Usage.OutputTokens, s.Usage.ReasoningTokens, s.Usage.CompleteAttempts+s.Usage.IncompleteAttempts > 0, s.Usage.CachedObserved, s.Usage.ReasoningObserved)
		if throughput := usagepresentation.OutputThroughputLabel(s.Usage.OutputThroughput); throughput != "" {
			add("Usage", "Average output", throughput)
			add("Usage", "Measured requests", s.Usage.OutputThroughput.MeasuredRequests)
			add("Usage", "Measured duration", fmt.Sprintf("%.3f s", float64(s.Usage.OutputThroughput.DurationNanos)/1e9))
		} else {
			add("Usage", "Average output", "unavailable")
		}
		add("Usage", "Throughput basis", "Measured output tokens / summed provider-request seconds, including latency and retries. Unmeasured requests excluded; not session wall time or mean request TPS.")
		add("Usage evidence", "", "Attempt coverage: complete categories, partial telemetry, or no usage observed.")
		add("Usage evidence", "Complete", s.Usage.CompleteAttempts)
		add("Usage evidence", "Partial", s.Usage.IncompleteAttempts)
		add("Usage evidence", "Missing", s.Usage.MissingAttempts)
		if s.Cache.ProviderCacheRate == nil {
			add("Cache", "Provider rate", "unavailable")
		} else {
			add("Cache", "Provider rate", fmt.Sprintf("%.1f%%", *s.Cache.ProviderCacheRate*100))
		}
		add("Cache", "", "Cached / input from observed usage. Partial or missing telemetry is not evidence of zero consumption or cache misses.")
		for _, row := range []struct {
			label string
			value uint64
		}{{"Records", s.Capture.Records}, {"Errors", s.Capture.CaptureErrors}, {"Incomplete", s.Capture.Incomplete}, {"Missing provider", s.Capture.MissingProvider}, {"Attempt gaps", s.Capture.AttemptGaps}, {"Write errors", s.Capture.WriteErrors}, {"Skipped", s.Capture.SkippedRequests}, {"Dropped detail", s.Capture.DroppedExchangeDetails}} {
			add("Capture", row.label, row.value)
		}
	case 1:
		add("Transport", "", "Exact wire bytes, including router-generated commentary.")
		for _, row := range []struct {
			label string
			bytes uint64
		}{
			{"Client requests", s.Transport.ClientRequests.Bytes},
			{"Provider requests", s.Transport.ProviderAttemptRequests.Bytes},
			{"Provider responses", s.Transport.ProviderResponses.Bytes},
			{"Client responses", s.Transport.ClientResponses.Bytes},
			{"Client control requests", s.Transport.ClientControlRequests.Bytes},
			{"Client control responses", s.Transport.ClientControlResponses.Bytes},
			{"Provider control requests", s.Transport.ProviderControlRequests.Bytes},
			{"Provider control responses", s.Transport.ProviderControlResponses.Bytes},
		} {
			payload("Transport", row.label, row.bytes)
		}
	case 2:
		if len(s.Exchanges) == 0 {
			add("Exchanges", "", "No completed exchanges yet. Active requests appear when they settle.")
			break
		}
		index := max(0, min(exchange, len(s.Exchanges)-1))
		e := s.Exchanges[index]
		add("Exchange", "Retained", fmt.Sprintf("%d / %d", index+1, len(s.Exchanges)))
		add("Exchange", "Sequence", e.Sequence)
		add("Exchange", "Thread", e.ThreadID)
		add("Exchange", "Model", e.Model)
		if e.Status != "" {
			add("Exchange", "Status", e.Status)
		}
		if e.RequestKind != "" {
			add("Exchange", "Kind", e.RequestKind)
		}
		add("Exchange", "Duration", fmt.Sprintf("%d ms", e.DurationMillis))
		if e.StatusCode != 0 {
			add("Exchange", "HTTP status", e.StatusCode)
		}
		add("Exchange", "Complete", e.ResponseComplete)
		add("Exchange", "Capture error", e.CaptureError)
		payload("Exchange", "Client request", e.ClientRequest.Bytes)
		payload("Exchange", "Client stream", e.ClientResponse.Bytes)
		if e.Usage == nil {
			add("Exchange", "Usage", "unavailable")
		} else {
			measuredUsage("Exchange", e.Usage.InputTokens, e.Usage.CachedInputTokens, e.Usage.UncachedInputTokens, e.Usage.OutputTokens, e.Usage.ReasoningTokens, e.Usage.CompleteAttempts+e.Usage.IncompleteAttempts > 0, e.Usage.CachedObserved, e.Usage.ReasoningObserved)
		}
		if d := e.CacheDiagnosis; d != nil {
			add("Cache diagnostics", "Previous", d.PreviousSequence)
			add("Cache diagnostics", "Client prefix", fmt.Sprintf("%s · %d common items · changed fields: %v", d.Client.Status, d.Client.CommonItems, d.Client.ChangedFields))
			add("Cache diagnostics", "Post replay", fmt.Sprintf("%s · %d common items · changed fields: %v", d.Projected.Status, d.Projected.CommonItems, d.Projected.ChangedFields))
			add("Cache diagnostics", "Provider prefix", fmt.Sprintf("%s · %d common items · changed fields: %v", d.Provider.Status, d.Provider.CommonItems, d.Provider.ChangedFields))
			add("Cache diagnostics", "Route key", d.Routing)
			add("Cache diagnostics", "Request key", d.RequestKey)
			add("Cache diagnostics", "Turn state", cmp.Or(d.TurnStateForwarding, "unavailable"))
		} else {
			add("Cache diagnostics", "", "Prefix comparisons unavailable on first or partial observations.")
		}
		if len(e.ProviderAttempts) == 0 {
			add("Provider attempts", "", "No provider attempt observed")
		}
		for _, a := range e.ProviderAttempts {
			group := fmt.Sprintf("Provider attempt %d", a.Attempt)
			add(group, "Model", a.Model)
			transport := cmp.Or(a.Transport, "http")
			if transport == "websocket" {
				transport = "WebSocket"
			} else if transport == "http" {
				transport = "HTTP"
			}
			add(group, "Transport", transport)
			if a.Status != "" {
				add(group, "Status", a.Status)
			}
			add(group, "Duration", fmt.Sprintf("%d ms", a.DurationMillis))
			if a.StatusCode != 0 {
				add(group, "HTTP status", a.StatusCode)
			}
			add(group, "Complete", a.ResponseComplete)
			add(group, "Capture error", a.CaptureError)
			payload(group, "Request", a.Request.Bytes)
			payload(group, "Stream", a.Response.Bytes)
			if a.Usage == nil {
				add(group, "Usage", "unavailable")
			} else {
				measuredUsage(group, a.Usage.InputTokens, a.Usage.CachedInputTokens, a.Usage.UncachedInputTokens, a.Usage.OutputTokens, a.Usage.ReasoningTokens, a.Usage.CompleteAttempts+a.Usage.IncompleteAttempts > 0, a.Usage.CachedObserved, a.Usage.ReasoningObserved)
				if a.Usage.CompleteAttempts != 0 {
					add(group, "Usage evidence", "complete")
				} else {
					add(group, "Usage evidence", "partial")
				}
			}
			if native {
				if a.ProviderResponse != nil {
					add(group, "Service tier", a.ProviderResponse.ServiceTier)
				}
				continue
			}
			if evidence := a.ProviderResponse; evidence != nil {
				add(group, "Service tier", cmp.Or(evidence.ServiceTier, "unavailable"))
				add(group, "Response model", cmp.Or(evidence.Model, "unavailable"))
				add(group, "Header model", cmp.Or(evidence.HeaderModel, "unavailable"))
				add(group, "Request ID", cmp.Or(evidence.RequestID, "unavailable"))
				add(group, "Cached telemetry", evidence.CachedTokensState)
				if evidence.CachedTokensState == "present" {
					optional(group, "Explicit cached", evidence.CachedTokens)
				}
			} else {
				add(group, "Service tier", "unavailable")
				add(group, "Telemetry", "unavailable")
			}
		}
	}
	return
}
