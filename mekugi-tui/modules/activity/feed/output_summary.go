package feed

// Summary bounds are content limits, independent of terminal width or reveal.
const SummaryLines = 5
const SummaryLineBytes = 512

// SummaryTail retains bounded logical source lines without an animation cursor.
type SummaryTail struct {
}
