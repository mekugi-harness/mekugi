package attachments

// SourceContent is one native reader result. Body bytes are not parsed as receipts.
type SourceContent struct{ Reference, Body, Error string }
