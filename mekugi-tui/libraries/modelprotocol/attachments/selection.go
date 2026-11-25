package attachments

// Selection holds quoted content separately from its visible delimiters.
type Selection struct {
	Label, Source        string
	Header, Body, Footer string
}
