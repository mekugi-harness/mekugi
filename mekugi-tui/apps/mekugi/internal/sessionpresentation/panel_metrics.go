package sessionpresentation

import (
	"cmp"
	"fmt"
)

// FeatureFields compares Self with Descendants in one table per feature family.
// Each family's first row is its total; later rows count subsets or failed attempts.
func FeatureFields(name string, f FeatureSnapshot) (fields []StatusField) {
	fields = append(fields, StatusField{Group: "Features", Value: fmt.Sprintf("Self is %s; Descendants sums its admitted agents (%d), including completed ones. Counts cover this launch and are not restored on resume.", cmp.Or(name, "the displayed thread"), len(f.Threads))})
	type row struct {
		label             string
		self, descendants any
	}
	table := func(group string, rows ...row) {
		fields = append(fields, StatusField{Group: group, Heading: true, Values: []string{"Self", "Descendants"}})
		for _, r := range rows {
			values := []string{fmt.Sprint(r.self), fmt.Sprint(r.descendants)}
			if f.Err != nil {
				values = []string{"unavailable", "unavailable"}
			}
			fields = append(fields, StatusField{Group: group, Label: r.label, Values: values})
		}
	}
	self, desc := f.Self, f.Descendants
	table("Compaction", row{"Total", self.Compactions, desc.Compactions}, row{"Self-initiated", self.SelfCompactions, desc.SelfCompactions}, row{"With preload", self.CompactionPreloads, desc.CompactionPreloads}, row{"Self completed", self.SelfCompactionsCompleted, desc.SelfCompactionsCompleted}, row{"Self failed", self.SelfCompactionFailures, desc.SelfCompactionFailures})
	table("Spawns", row{"Total", self.Spawns, desc.Spawns}, row{"With preload", self.SpawnPreloads, desc.SpawnPreloads}, row{"Preload rejected", self.PreloadRejections, desc.PreloadRejections})
	table("Delegate finals", row{"Completed", self.Finals, desc.Finals}, row{"With edit ranges", self.CompletionChanges, desc.CompletionChanges})
	table("Messages", row{"Sent", self.Messages, desc.Messages}, row{"With btw", self.SideQuestions, desc.SideQuestions}, row{"btw rejected", self.SideQuestionRejections, desc.SideQuestionRejections}, row{"btw failed", self.SideQuestionFailures, desc.SideQuestionFailures})
	table("Waits", row{"Registered", self.Waits, desc.Waits}, row{"All mode", self.AllWaits, desc.AllWaits}, row{"Progress enabled", self.ProgressWaits, desc.ProgressWaits}, row{"Progress delivered", self.ProgressYields, desc.ProgressYields})
	table("Source-bearing sends", row{"Total", self.AssignmentSources + self.MessageSources, desc.AssignmentSources + desc.MessageSources}, row{"Assignments", self.AssignmentSources, desc.AssignmentSources}, row{"Messages", self.MessageSources, desc.MessageSources})
	table("Output deduplication", row{"Saved bytes", self.OutputDeduplicationBytes, desc.OutputDeduplicationBytes}, row{"Saved tokens (est.)", self.OutputDeduplicationTokens, desc.OutputDeduplicationTokens})
	fields = append(fields, StatusField{Group: "Output deduplication", Value: "Net model-input savings after reference overhead, summed across requests. Tokens use a local 4-byte estimate, not provider billing savings."})
	return fields
}
