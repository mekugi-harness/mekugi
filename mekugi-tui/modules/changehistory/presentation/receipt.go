package presentation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
)

// ReceiptEvidence contains captured facts needed for a source-text edit summary.
// It carries no request, backend or retained-store handles.
type ReceiptEvidence struct {
	Files []diffmodel.ReviewFile
	// Counts preserve exact totals when Files contains detached bounded excerpts.
	Counts [][2]int
	// Truncated marks excerpts whose full omitted-row count is unavailable.
	Truncated                  []bool
	Labels, DeletedDirectories []string
	Source, ChangeID           string
}

// Source: apps/mekugi/internal/router/native_edit_receipt.go@6f68e18a.
// Receipt diffs are display-only and stay well inside one publication.
const (
	MaxReceiptDiffBytes     = 8 << 10
	maxEditReceiptFileLines = 80
)
