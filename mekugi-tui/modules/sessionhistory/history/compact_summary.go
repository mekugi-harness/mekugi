// Read-only support for historical retained compaction summary records.
package history

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
)

const CompactSummaryPrefix = "mekugi:compact:"

var CompactSummaryRecord = recordkind.Spec{ID: "compact", Match: recordkind.HashedName("compact-", ".json")}

type CompactRecord struct {
	Bytes              int `json:"-"` // Retained input charged by bounded consumers.
	Version            int
	Reference, Summary string
	Selection          string
}
