package sessionreview

import (
	"cmp"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/activitydetail"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

// nativeCaptureOrderBase orders native store captures deterministically after
// legacy sequence numbers while preserving strict native sequence order.
const nativeCaptureOrderBase uint64 = 1 << 62

// NativeStoreDir resolves the durable native change-history directory for a
// Codex session ID, matching codex-rs/core/src/exec_env.rs change_history_dir.
func NativeStoreDir(sessionID string) string {
	if sessionID == "" || strings.ContainsAny(sessionID, "/\\") || sessionID == "." || sessionID == ".." {
		return ""
	}
	return filepath.Join(os.TempDir(), "mekugi-session-"+sessionID, "change-history")
}

type nativeStoreIndex struct {
	SchemaVersion uint32             `json:"schema_version"`
	NextSequence  uint64             `json:"next_sequence"`
	ThreadStreams map[string]string  `json:"thread_streams"`
	Entries       []nativeIndexEntry `json:"entries"`
}

type nativeIndexEntry struct {
	ID                    string   `json:"id"`
	Sequence              uint64   `json:"sequence"`
	ThreadID              string   `json:"thread_id"`
	CallID                string   `json:"call_id"`
	SourceKind            string   `json:"source_kind"`
	CreatedAt             int64    `json:"created_at"`
	ExitCode              *int32   `json:"exit_code,omitempty"`
	FilePaths             []string `json:"file_paths"`
	HasIncompleteCoverage bool     `json:"has_incomplete_coverage"`
}

type nativeChangeRecord struct {
	SchemaVersion uint32               `json:"schema_version"`
	ID            string               `json:"id"`
	Sequence      uint64               `json:"sequence"`
	Metadata      nativeChangeMetadata `json:"metadata"`
	Files         []nativeFileEvidence `json:"files"`
}

type nativeChangeMetadata struct {
	ThreadID      string `json:"thread_id"`
	SessionID     string `json:"session_id"`
	TurnID        string `json:"turn_id"`
	CallID        string `json:"call_id"`
	SourceKind    string `json:"source_kind"`
	WorkspaceRoot string `json:"workspace_root"`
	CreatedAt     int64  `json:"created_at"`
	ExitCode      *int32 `json:"exit_code,omitempty"`
	Summary       string `json:"summary,omitempty"`
	ClaimKey      string `json:"claim_key,omitempty"`
}

type nativeFileEvidence struct {
	RelPath   string               `json:"rel_path"`
	AbsPath   string               `json:"abs_path"`
	MovedFrom string               `json:"moved_from,omitempty"`
	Kind      nativeFileChangeKind `json:"kind"`
	Before    nativeFileState      `json:"before"`
	After     nativeFileState      `json:"after"`
	Coverage  nativeCoverageStatus `json:"coverage"`
}

type nativeFileChangeKind struct {
	Kind string `json:"kind"`
	From string `json:"from,omitempty"`
}

type nativeFileState struct {
	State    string `json:"state"`
	BlobHash string `json:"blob_hash,omitempty"`
	ByteLen  int64  `json:"byte_len,omitempty"`
	IsBinary bool   `json:"is_binary,omitempty"`
}

type nativeCoverageStatus struct {
	Status string `json:"status"`
	Reason string `json:"reason,omitempty"`
}

func isValidNativeChangeID(id string) bool {
	if id == "" {
		return false
	}
	i := 0
	for i < len(id) && id[i] >= 'a' && id[i] <= 'z' {
		i++
	}
	if i == 0 || i == len(id) {
		return false
	}
	for ; i < len(id); i++ {
		if id[i] < '0' || id[i] > '9' {
			return false
		}
	}
	return true
}

func isValidSHA256Hex(s string) bool {
	if len(s) != 64 {
		return false
	}
	for i := range len(s) {
		c := s[i]
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')) {
			return false
		}
	}
	return true
}

func nativeStreamIndex(stream string) int {
	idx := 0
	for i := range len(stream) {
		c := stream[i]
		if c < 'a' || c > 'z' {
			return 0
		}
		idx = idx*26 + int(c-'a') + 1
	}
	return idx
}

func admittedNativeWorkspace(scope previewmodel.Scope, thread, preferred string) string {
	if preferred != "" {
		clean := filepath.Clean(preferred)
		if scope.Workspaces[clean][thread] {
			return clean
		}
	}
	var fallback string
	for _, workspace := range slices.Sorted(maps.Keys(scope.Workspaces)) {
		if !scope.Workspaces[workspace][thread] {
			continue
		}
		if preferred != "" && changes.PathWithin(filepath.Clean(preferred), filepath.Clean(workspace)) {
			return workspace
		}
		if fallback == "" {
			fallback = workspace
		}
	}
	return fallback
}

func threadInScope(scope previewmodel.Scope, thread string) bool {
	if thread == "" {
		return false
	}
	for _, threads := range scope.Workspaces {
		if threads[thread] {
			return true
		}
	}
	return false
}

func singleAdmittedWorkspace(scope previewmodel.Scope, thread string) string {
	var selected string
	for workspace, threads := range scope.Workspaces {
		if !threads[thread] {
			continue
		}
		if selected != "" && selected != workspace {
			return ""
		}
		selected = workspace
	}
	return selected
}

func collectLegacyCalls(snapshot *presentation.Snapshot) map[[2]string]bool {
	calls := make(map[[2]string]bool)
	if snapshot == nil {
		return calls
	}
	for attempt := range snapshot.Attempts() {
		if strings.HasPrefix(attempt.Key.Call, "native:") {
			continue
		}
		recordLegacyAttemptCalls(calls, attempt)
	}
	return calls
}

func recordLegacyAttemptCalls(calls map[[2]string]bool, attempt presentation.Attempt) {
	if calls == nil {
		return
	}
	if attempt.Operation.Thread != "" && attempt.Operation.Item != "" {
		calls[[2]string{attempt.Operation.Thread, attempt.Operation.Item}] = true
	}
	if attempt.Receipt != nil && attempt.Receipt.Thread != "" {
		for _, call := range attempt.Receipt.Calls {
			if call != "" {
				calls[[2]string{attempt.Receipt.Thread, call}] = true
			}
		}
	}
}

// reconcileNativeReview hydrates published records from the native Rust
// change-history store into builder for threads admitted in scope.
func reconcileNativeReview(
	ctx context.Context,
	dir string,
	callers map[string]string,
	builder *presentation.Builder,
	scope previewmodel.Scope,
	legacyCalls map[[2]string]bool,
	present map[presentation.AttemptKey]bool,
) (bool, error) {
	if dir == "" || len(scope.Workspaces) == 0 {
		return false, nil
	}
	unlock, err := lockNativeStoreShared(dir)
	if err != nil {
		return false, err
	}
	defer unlock()

	indexBytes, err := os.ReadFile(filepath.Join(dir, "index.json"))
	if err != nil {
		if os.IsNotExist(err) {
			return false, nil
		}
		return false, err
	}
	var index nativeStoreIndex
	if err := json.Unmarshal(indexBytes, &index); err != nil {
		return false, fmt.Errorf("native change-history index: %w", err)
	}

	remainingBytes := changes.MaxReadBytes
	added := false
	for _, entry := range index.Entries {
		if err := ctx.Err(); err != nil {
			return false, err
		}
		if !isValidNativeChangeID(entry.ID) {
			return false, fmt.Errorf("native change-history has invalid change ID %q", entry.ID)
		}
		if !threadInScope(scope, entry.ThreadID) {
			continue
		}
		if entry.CallID != "" && legacyCalls[[2]string{entry.ThreadID, entry.CallID}] {
			continue
		}

		callKey := "native:" + entry.ID
		streamIdx := nativeStreamIndex(index.ThreadStreams[entry.ThreadID])
		correlation := cmp.Or(entry.CallID, entry.ID)

		if singleWS := singleAdmittedWorkspace(scope, entry.ThreadID); singleWS != "" && filepath.IsAbs(singleWS) {
			identity := presentation.AttemptIdentity{
				Key:         presentation.AttemptKey{Workspace: singleWS, Call: callKey},
				Change:      entry.ID,
				Correlation: correlation,
				Stream:      streamIdx,
			}
			if found, err := builder.Known(identity); err != nil {
				return false, err
			} else if found {
				if present != nil {
					present[identity.Key] = true
				}
				continue
			}
		}

		recordBytes, err := os.ReadFile(filepath.Join(dir, "records", entry.ID+".json"))
		if err != nil {
			return false, fmt.Errorf("native change %s: %w", entry.ID, err)
		}
		var record nativeChangeRecord
		if err := json.Unmarshal(recordBytes, &record); err != nil {
			return false, fmt.Errorf("native change %s: %w", entry.ID, err)
		}
		if record.ID != entry.ID {
			return false, fmt.Errorf("native change %s: record ID mismatch (%q)", entry.ID, record.ID)
		}

		workspace := admittedNativeWorkspace(scope, record.Metadata.ThreadID, record.Metadata.WorkspaceRoot)
		if workspace == "" || !filepath.IsAbs(workspace) {
			continue
		}
		identity := presentation.AttemptIdentity{
			Key:         presentation.AttemptKey{Workspace: workspace, Call: callKey},
			Change:      record.ID,
			Correlation: cmp.Or(record.Metadata.CallID, record.ID),
			Stream:      streamIdx,
		}
		if present != nil {
			present[identity.Key] = true
		}
		if found, err := builder.Known(identity); err != nil {
			return false, err
		} else if found {
			continue
		}

		recordWorkspace := cmp.Or(record.Metadata.WorkspaceRoot, workspace)
		caller := callers[record.Metadata.ThreadID]
		if caller == "" {
			caller = "/root"
		}
		source := nativeOriginSource(record.Metadata.SourceKind)
		origin := presentation.Origin{Change: record.ID, Caller: caller, Source: source}
		attempt := presentation.Attempt{
			AttemptIdentity: identity,
			Operation:       activitydetail.Operation{Thread: record.Metadata.ThreadID, Item: record.Metadata.CallID},
		}
		captureOrder := nativeCaptureOrderBase + record.Sequence
		streamKey := workspace + "\x00native\x00" + record.Metadata.ThreadID
		for n, file := range record.Files {
			if err := ctx.Err(); err != nil {
				return false, err
			}
			reviewFile, err := buildNativeReviewFile(dir, record.ID, recordWorkspace, record.Metadata.SourceKind, file, &remainingBytes)
			if err != nil {
				return false, err
			}
			attempt.Chunks = append(attempt.Chunks, presentation.Chunk{
				Key:          workspace + "\x00" + callKey + "/" + strconv.Itoa(n),
				Stream:       streamKey,
				Workspace:    workspace,
				CaptureOrder: captureOrder,
				Review:       reviewFile,
				Origin:       origin,
			})
		}
		if err := builder.Add(attempt); err != nil {
			return false, err
		}
		added = true
	}
	return added, nil
}

func nativeOriginSource(sourceKind string) string {
	switch sourceKind {
	case "":
		return "apply_patch"
	case "user_shell":
		return "userShell"
	default:
		return sourceKind
	}
}

func buildNativeReviewFile(
	dir, recordID, workspace, sourceKind string,
	file nativeFileEvidence,
	remainingBytes *int,
) (diffmodel.ReviewFile, error) {
	canonical := func(path string) string {
		if path == "" {
			return ""
		}
		if !filepath.IsAbs(path) && workspace != "" {
			path = filepath.Join(workspace, path)
		}
		return filepath.Clean(path)
	}

	absPath := canonical(cmp.Or(file.AbsPath, file.RelPath))
	var beforePath, afterPath string
	switch file.Kind.Kind {
	case "add":
		beforePath, afterPath = "", absPath
	case "delete":
		beforePath, afterPath = absPath, ""
	case "move":
		from := cmp.Or(file.MovedFrom, file.Kind.From)
		beforePath, afterPath = canonical(from), absPath
	default:
		if file.MovedFrom != "" {
			beforePath, afterPath = canonical(file.MovedFrom), absPath
		} else {
			beforePath, afterPath = absPath, absPath
		}
	}

	var review diffmodel.ReviewFile
	switch file.Coverage.Status {
	case "missing_baseline":
		reason := cmp.Or(file.Coverage.Reason, "baseline unavailable")
		if file.After.State == "present" && !file.After.IsBinary && afterPath != "" {
			afterBytes, err := readNativeBlob(dir, recordID, file.After, remainingBytes)
			if err != nil {
				return diffmodel.ReviewFile{}, err
			}
			review = diffmodel.RenderUnbasedReviewFile(afterPath, string(afterBytes), reason)
		} else {
			review = diffmodel.RenderIncompleteReviewFile(beforePath, afterPath, reason)
		}
	case "partial":
		review = diffmodel.RenderIncompleteReviewFile(beforePath, afterPath, cmp.Or(file.Coverage.Reason, "partial capture coverage"))
	default:
		if file.Before.IsBinary || file.After.IsBinary {
			review = diffmodel.RenderBinaryReviewFile(
				beforePath, afterPath,
				file.Before.ByteLen, file.After.ByteLen,
				file.Before.BlobHash, file.After.BlobHash,
			)
		} else {
			beforeBytes, err := readNativeBlob(dir, recordID, file.Before, remainingBytes)
			if err != nil {
				return diffmodel.ReviewFile{}, err
			}
			afterBytes, err := readNativeBlob(dir, recordID, file.After, remainingBytes)
			if err != nil {
				return diffmodel.ReviewFile{}, err
			}
			review = diffmodel.RenderReviewFile(beforePath, afterPath, string(beforeBytes), string(afterBytes))
		}
	}

	if sourceKind == "vcs_diagnostic" {
		review.Origin = "git"
	}
	return review, nil
}

func readNativeBlob(dir, recordID string, state nativeFileState, remainingBytes *int) ([]byte, error) {
	if state.State != "present" {
		return nil, nil
	}
	if !isValidSHA256Hex(state.BlobHash) {
		return nil, fmt.Errorf("native change %s: invalid blob hash %q", recordID, state.BlobHash)
	}
	if state.ByteLen < 0 || state.ByteLen > int64(*remainingBytes) {
		return nil, errors.New("live diff exceeds 64 MiB; use mchanges with a narrower range")
	}
	data, err := os.ReadFile(filepath.Join(dir, "blobs", state.BlobHash))
	if err != nil {
		return nil, fmt.Errorf("native change %s: read blob %s: %w", recordID, state.BlobHash, err)
	}
	if int64(len(data)) != state.ByteLen {
		return nil, fmt.Errorf("native change %s: blob %s length mismatch", recordID, state.BlobHash)
	}
	sum := sha256.Sum256(data)
	if hex.EncodeToString(sum[:]) != state.BlobHash {
		return nil, fmt.Errorf("native change %s: blob %s checksum mismatch", recordID, state.BlobHash)
	}
	*remainingBytes -= len(data)
	return data, nil
}
