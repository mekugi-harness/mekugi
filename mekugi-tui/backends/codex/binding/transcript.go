package binding

// TranscriptItem is the decoded native item's text subset. Observation excludes
// raw/private reasoning; user content remains with the Conversation consumer.
type TranscriptItem struct {
	ID, Type, Text, Status, Delivery, Phase string
	Summary                                 []string
	Questions                               []Question
}
