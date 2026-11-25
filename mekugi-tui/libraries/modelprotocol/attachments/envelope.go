// Package attachments encodes submitted snapshots carried in host history.
// Source: internal/router/composer_file_attachment.go:15:47@b9a7b3fe;
// internal/router/app_server_text_spans.go:40:74@b9a7b3fe;
// internal/router/composer_selection.go:124:129@b9a7b3fe.
package attachments

// The envelope travels through Codex's own history. Projection never reopens a
// file, so forks, resume, and retries see the submitted snapshot, not today's file.
const Prefix = "<mekugi-file-attachments-v1>\n"
const Suffix = "\n</mekugi-file-attachments-v1>"

const ReceiptPrefix = "attachment\x00"

const ReadGuidance = "use supplied content; reread only for edits or missing/newer content"

const (
	Budget = 192 << 10 // Encoded envelope bytes, including JSON escaping.
	Chunk  = 24 << 10  // Raw UTF-8 bytes per framed model message.
)

// Snapshot retains submission identity independently of frame contents. Legacy
// envelopes have no identity and must not be correlated by content or position.
type Snapshot struct {
	ID     string   `json:"id"`
	Frames []string `json:"frames"`
}

// SelectionPrefix starts historical selection frames and current omission notices.
const SelectionPrefix = "Selected text for "
