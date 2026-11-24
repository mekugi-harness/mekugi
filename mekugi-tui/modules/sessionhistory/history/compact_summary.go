// Read-only support for historical retained compaction summary records.
package history

import (
	"crypto/sha256"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
)

const CompactSummaryPrefix = "mekugi:compact:"

func CompactSummaryName(reference string) string {
	return fmt.Sprintf("compact-%x.json", sha256.Sum256([]byte(reference)))
}

var CompactSummaryRecord = recordkind.Spec{ID: "compact", Match: recordkind.HashedName("compact-", ".json")}

type CompactRecord struct {
	Bytes              int `json:"-"` // Retained input charged by bounded consumers.
	Version            int
	Reference, Summary string
	Selection          string
}

func DecodeCompactSummary(data []byte, reference string) (CompactRecord, error) {
	var r CompactRecord
	if err := json.Unmarshal(data, &r); err != nil {
		r.Bytes = len(data)
		return r, err
	}
	r.Bytes = len(data)
	if r.Version != 1 || r.Reference != reference || strings.TrimSpace(r.Summary) == "" {
		return r, errors.New("invalid retained compaction summary")
	}
	return r, nil
}

// ReadCompactSummary resolves an exact reference using the requesting session's
// frozen handle scope. It starts no acquisition and never creates or extends a scope.
func ReadCompactSummary(store *Store, thread, reference string) (string, int, error) {
	if store == nil || thread == "" || !strings.HasPrefix(reference, CompactSummaryPrefix) {
		return "", 0, errors.New("compaction summary is unavailable")
	}
	namespace, err := store.NamespaceForThread(thread)
	if err != nil {
		return "", 0, err
	}
	record, err := ReadCompactRecord(store, namespace, reference)
	return record.Summary, record.Bytes, err
}

// ReadCompactRecord validates the retained reference within an already admitted namespace.
func ReadCompactRecord(store *Store, namespace, reference string) (CompactRecord, error) {
	if store == nil || namespace == "" || !strings.HasPrefix(reference, CompactSummaryPrefix) {
		return CompactRecord{}, errors.New("compaction summary is unavailable")
	}
	source, handle, ok := strings.Cut(strings.TrimPrefix(reference, CompactSummaryPrefix), ":")
	owner, err := store.HandleOwner(namespace, handle)
	if err != nil || !ok || owner != source {
		return CompactRecord{}, errors.New("compaction summary is unavailable in this session")
	}
	data, err := ReadRecord(filepath.Join(store.Directory(), CompactSummaryName(reference)))
	if err != nil {
		return CompactRecord{Bytes: len(data)}, err
	}
	return DecodeCompactSummary(data, reference)
}
