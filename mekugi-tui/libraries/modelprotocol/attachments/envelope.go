// Package attachments encodes submitted snapshots carried in host history.
// Source: internal/router/composer_file_attachment.go:15:47@b9a7b3fe;
// internal/router/app_server_text_spans.go:40:74@b9a7b3fe;
// internal/router/composer_selection.go:124:129@b9a7b3fe.
package attachments

const Budget = 192 << 10 // Encoded envelope bytes, including JSON escaping.

// Snapshot retains submission identity independently of frame contents. Legacy
// envelopes have no identity and must not be correlated by content or position.
type Snapshot struct {
	ID     string   `json:"id"`
	Frames []string `json:"frames"`
}
